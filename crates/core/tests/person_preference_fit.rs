//! `PersonPreferenceFit`: the per-placement soft term.
//!
//! Named for the behaviour rather than for a slice, because that is what a
//! maintainer looking for it will search for.
//!
//! Two of these tests carry the weight, and both are written to be **red
//! against a plausible wrong implementation** rather than merely green against
//! the right one:
//!
//! * `the_cost_is_the_mean_of_the_product` pins the combination rule over a
//!   placement's lecturers. Three candidate rules are defensible on a skim and
//!   two are wrong; a fixture with equal multipliers or equal fits makes all
//!   three agree, so the fixture is built so they disagree.
//! * `incremental_objective_matches_full_recomputation` extends the drift
//!   assertion to a term the existing seeded instances never carried. The
//!   preference cost is delta-accumulated into `Objective::soft`, so a
//!   `place`/`unplace` pair that computed it differently would leave the search
//!   optimizing a number that no longer describes the schedule — and every
//!   other test in this file would still pass.

use calendry_solver_core::ids::{PlacementIdx, SlotIdx};
use calendry_solver_core::preferences::{
    MAX_WEIGHT_MULTIPLIER, MIN_WEIGHT_MULTIPLIER, PreferenceModel,
};
use calendry_solver_core::problem::ProblemSpec;
use calendry_solver_core::search::{
    NeverHalt, objectives_agree, recompute_objective, soft_breakdown, solve,
};
use calendry_solver_core::testing::{self, PREFERENCE_WEIGHT};
use calendry_solver_core::{Problem, Solution};

mod common;
use common::{SEED, moves, solve_with_move_budget as run};

/// Monday is slot 0 and Saturday is slot 1 in `two_day_grid`.
const MONDAY: SlotIdx = SlotIdx(0);
const SATURDAY: SlotIdx = SlotIdx(1);
const ONLY: PlacementIdx = PlacementIdx(0);

fn cost_at(problem: &Problem, slot: SlotIdx) -> f64 {
    problem.preferences.cost(ONLY, slot, &[])
}

// ---------------------------------------------------------------------------
// The combination rule over a placement's lecturers
// ---------------------------------------------------------------------------

#[test]
fn the_cost_is_the_mean_of_the_product() {
    // Two required lecturers whose multipliers AND whose satisfaction differ:
    // multipliers 0.5 and 2.0, and on Monday the first is satisfied while the
    // second is not. Both halves of that are necessary — with equal multipliers
    // or equal fits, mean, sum and the separated product all agree and the test
    // proves nothing.
    let problem = testing::two_lecturers_with_opposing_preferences();
    let w = PREFERENCE_WEIGHT;

    // MONDAY: the 0.5 lecturer got their day, the 2.0 lecturer did not.
    //   mean(m * unmet) = (0.5*0.0 + 2.0*1.0) / 2 = 1.0
    assert_eq!(cost_at(&problem, MONDAY), 1.0 * w, "Monday: mean of the product");

    // SATURDAY: the reverse.
    //   mean(m * unmet) = (0.5*1.0 + 2.0*0.0) / 2 = 0.25
    assert_eq!(cost_at(&problem, SATURDAY), 0.25 * w, "Saturday: mean of the product");
}

#[test]
fn the_sum_form_would_have_produced_a_different_number() {
    // Summing instead of averaging doubles both numbers here. It is bounded by
    // `max_multiplier * |P|`, which puts an instance-data quantity —  how many
    // lecturers this Offering happens to require — back into the hard-penalty
    // ceiling, so a tenant could raise this type's contribution arbitrarily by
    // raising `required_lecturer_count`, with no weight change and no warning.
    let problem = testing::two_lecturers_with_opposing_preferences();
    let w = PREFERENCE_WEIGHT;

    assert_ne!(cost_at(&problem, MONDAY), 2.0 * w, "must not be the sum over lecturers");
    assert_ne!(cost_at(&problem, SATURDAY), 0.5 * w, "must not be the sum over lecturers");
}

#[test]
fn the_separated_form_would_have_lost_the_preference_entirely() {
    // `mean(m) * mean(unmet)` = 1.25 * 0.5 = 0.625 on BOTH days. This is the
    // wrong form a reader is most likely to write from skimming the formula, and
    // the one a bound check cannot catch: `mean(m) <= 2.0` and
    // `mean(unmet) <= 1.0`, so it respects the ceiling too.
    //
    // What it does not respect is attribution. It applies the AVERAGE
    // multiplier to the AVERAGE fit, so the lecturer with the 2.0 multiplier
    // inflates the cost even on the day that suits them perfectly — charging
    // the institution for caring a lot about someone who got what they wanted.
    let problem = testing::two_lecturers_with_opposing_preferences();
    let w = PREFERENCE_WEIGHT;

    assert_ne!(cost_at(&problem, MONDAY), 0.625 * w, "must not be mean(m) * mean(unmet)");

    // And the sharper half of it: under the separated form the two days cost
    // the same, so the term would express no preference between them at all.
    assert_ne!(
        cost_at(&problem, MONDAY),
        cost_at(&problem, SATURDAY),
        "the two days must be priced differently, or the term steers nothing"
    );
}

#[test]
fn the_search_places_where_the_cheaper_day_is() {
    // The direction test: it is not enough to compute a number. Saturday costs
    // 0.25w against Monday's 1.0w, one room, one Session — so the search must
    // land on Saturday, and the lecturer with the 2.0 multiplier is the one who
    // gets their way.
    let problem = testing::two_lecturers_with_opposing_preferences();
    let outcome = run(&problem);

    let at = outcome
        .solution
        .get(ONLY)
        .expect("one Session, two slots, one room");
    assert_eq!(at.start, SATURDAY, "should honour the higher-weighted lecturer");
    assert_eq!(outcome.objective.soft, 0.25 * PREFERENCE_WEIGHT);
}

// ---------------------------------------------------------------------------
// Zero is the identity, and two different facts reduce to it
// ---------------------------------------------------------------------------

#[test]
fn no_counted_lecturer_costs_nothing_from_an_all_zero_row() {
    // `|P| = 0` is reachable, not theoretical: `required_lecturer_count`
    // defaults to 0, so a tenant-defined `staff_meeting` kind requires no
    // lecturer. The mean is undefined there.
    //
    // The fixture's one Person has a preference AND a 2.0 multiplier, so a
    // scoring path that fell back to "everyone in the tenant" rather than "this
    // placement's lecturers" would charge here.
    let problem = testing::no_lecturers_with_preference_enabled();

    assert_eq!(cost_at(&problem, MONDAY), 0.0);
    assert_eq!(cost_at(&problem, SATURDAY), 0.0);

    // Resolved at table-build time, not at read time: the row is zeros rather
    // than the read being conditional, which is what keeps the scoring path a
    // branch-free indexed read.
    assert_eq!(problem.preferences.unmet(ONLY, MONDAY, &[]), 0.0);
}

#[test]
fn a_lecturer_who_stated_nothing_is_not_counted() {
    // The distinction the whole term rests on: "stated nothing" must cost
    // nothing, while "stated something and did not get it" must cost. An
    // implementation that gave a preference-less lecturer a fit of 0 and left
    // them in the counted set would charge every placement everywhere, and the
    // rule would look like it was working.
    let problem = one_session_with_lecturers(vec![
        testing::person_with_preference("states", &[], testing::preference(&[1], &[], None)),
        testing::person("silent", &[]),
    ]);

    // Only the stating lecturer counts, so Monday is fully satisfied and
    // Saturday is fully unsatisfied — 1.0, not the 0.5 an average over both
    // people would give.
    assert_eq!(cost_at(&problem, MONDAY), 0.0);
    assert_eq!(cost_at(&problem, SATURDAY), 1.0 * PREFERENCE_WEIGHT);
}

#[test]
fn a_disabled_rule_is_inert_even_with_preferences_on_file() {
    // The tenant-switch half of the `LecturerVeto` architecture: the values live
    // on the Person and are read only when the tenant enables the rule.
    let with = testing::two_lecturers_with_opposing_preferences();
    let without = one_session_with_lecturers_and_rule(
        vec![
            testing::person_with_preference("half", &[], testing::preference(&[1], &[], Some(0.5))),
            testing::person_with_preference(
                "double",
                &[],
                testing::preference(&[6], &[], Some(2.0)),
            ),
        ],
        Vec::new(),
    );

    assert_eq!(without.preferences.cost(ONLY, MONDAY, &[]), 0.0);
    assert_eq!(without.preferences.cost(ONLY, SATURDAY, &[]), 0.0);
    assert!(without.preferences.is_empty());

    // And the objective differs, so the rule is not merely reporting: the
    // enabled run pays for Monday and the disabled one does not.
    assert!(with.preferences.cost(ONLY, MONDAY, &[]) > 0.0);
}

// ---------------------------------------------------------------------------
// Narrowing: legitimate data this grid has no slot for
// ---------------------------------------------------------------------------

#[test]
fn a_value_outside_this_grid_is_dropped_rather_than_unsatisfiable() {
    // The app validates a preference against the tenant's WIDEST grid, because
    // a preference is not term-scoped, while exactly one grid is in force at
    // solve time. So a stored `block 9` on a 1-block grid, or a Wednesday on a
    // Monday/Saturday grid, is legitimate data and an impossible slot at once.
    //
    // Dropped, matching `MinimizeBlockUsage`, where a stale index "simply never
    // matches". Keeping it would do the OPPOSITE of inert: an axis that can
    // never be satisfied charges the person at every slot in the grid, with no
    // placement able to fix it.
    let problem = one_session_with_lecturers(vec![testing::person_with_preference(
        "wednesday",
        &[],
        // Wednesday is not an active day here; block 9 is past the end of the
        // day. Nothing usable is left.
        testing::preference(&[3], &[9], None),
    )]);

    assert_eq!(cost_at(&problem, MONDAY), 0.0, "an unsatisfiable axis must not charge");
    assert_eq!(cost_at(&problem, SATURDAY), 0.0);
}

#[test]
fn a_partly_narrowed_preference_keeps_the_axis_that_survived() {
    // Half the statement is expressible on this grid. The divisor must be the
    // number of axes that SURVIVED narrowing, not the number stated — otherwise
    // Monday would price at 0.5 rather than 0.0 and this person could never be
    // fully satisfied by any placement.
    let problem = one_session_with_lecturers(vec![testing::person_with_preference(
        "monday-and-nonsense",
        &[],
        testing::preference(&[1], &[9], None),
    )]);

    assert_eq!(cost_at(&problem, MONDAY), 0.0, "the surviving axis is fully satisfiable");
    assert_eq!(cost_at(&problem, SATURDAY), 1.0 * PREFERENCE_WEIGHT);
}

#[test]
fn both_axes_earn_credit_independently() {
    // ADDITIVE partial credit: `{days: [1], blocks: [1]}` reads as two separate
    // statements — "I like Mondays, and I like second blocks" — so a Monday
    // second block earns both and a Monday first block earns half. Not a
    // conjunction, which the two-array shape cannot express.
    let problem = one_session_on_grid(
        testing::grid(2, 1), // Monday only, two blocks
        vec![testing::person_with_preference(
            "additive",
            &[],
            testing::preference(&[1], &[1], None),
        )],
    );
    let w = PREFERENCE_WEIGHT;

    // Monday block 0: day matched, block did not.
    assert_eq!(problem.preferences.cost(ONLY, SlotIdx(0), &[]), 0.5 * w, "one axis of two");
    // Monday block 1: both matched.
    assert_eq!(problem.preferences.cost(ONLY, SlotIdx(1), &[]), 0.0, "both axes");
}

// ---------------------------------------------------------------------------
// The multiplier, and the bound it must not escape
// ---------------------------------------------------------------------------

#[test]
fn the_multiplier_is_clamped_on_read() {
    // The app validates the range at its write boundary and a database CHECK
    // backs it up, but this service accepts possibly-invalid input by design —
    // and this is the one field whose value feeds the derived `hard_penalty`
    // bound. An unclamped 100.0 here would let a tenant-editable column decide
    // whether an unplaced Session still outranks a bad soft configuration.
    for (given, expected) in [
        (100.0, MAX_WEIGHT_MULTIPLIER),
        (0.01, MIN_WEIGHT_MULTIPLIER),
    ] {
        let problem = one_session_with_lecturers(vec![testing::person_with_preference(
            "out-of-range",
            &[],
            testing::preference(&[1], &[], Some(given)),
        )]);
        assert_eq!(
            problem.preferences.cost(ONLY, SATURDAY, &[]),
            expected * PREFERENCE_WEIGHT,
            "multiplier {given} should clamp to {expected}"
        );
    }
}

#[test]
fn a_non_finite_multiplier_does_not_poison_the_objective() {
    // `f64::clamp` PROPAGATES NaN rather than clamping it, so a NaN reaching the
    // table would make the whole objective NaN — and every comparison in the
    // search silently false. Replaced outright instead.
    for given in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let problem = one_session_with_lecturers(vec![testing::person_with_preference(
            "nonsense",
            &[],
            testing::preference(&[1], &[], Some(given)),
        )]);
        let cost = problem.preferences.cost(ONLY, SATURDAY, &[]);
        assert!(cost.is_finite(), "{given} produced {cost}");
        assert_eq!(cost, PREFERENCE_WEIGHT, "should fall back to a 1.0 multiplier");
    }
}

#[test]
fn one_unplaced_session_still_outranks_every_preference_cost() {
    // The lexicographic property, with this term's ceiling in it. `hard_penalty`
    // has to account for `weight * MAX_WEIGHT_MULTIPLIER` per placement, not
    // `weight` — summing raw weights would leave the bound short by exactly the
    // multiplier and a heavily-preferred schedule could outrank a hole in the
    // timetable.
    let problem = testing::two_lecturers_with_opposing_preferences();

    let worst = problem.preferences.max_cost_per_placement() * problem.placements.len() as f64;
    assert!(
        problem.hard_penalty > worst,
        "hard_penalty {} must exceed the worst reachable preference total {worst}",
        problem.hard_penalty
    );

    // And the ceiling is computable from the configuration alone: adding a
    // Person with an override on file must not move it. The `* 2.0` is
    // day/block and room counted as two independent additive families, each
    // bounded by `MAX_WEIGHT_MULTIPLIER` on its own — see
    // `PREFERENCE_AXIS_FAMILIES` in `preferences.rs`.
    assert_eq!(
        problem.preferences.max_cost_per_placement(),
        PREFERENCE_WEIGHT * MAX_WEIGHT_MULTIPLIER * 2.0
    );
}

// ---------------------------------------------------------------------------
// Room-type preference — the second, independent additive term
// ---------------------------------------------------------------------------

#[test]
fn a_room_preference_is_met_by_the_room_that_has_the_feature() {
    // R0 has "lab", R1 does not — the fixture's whole point.
    let problem = testing::one_lecturer_wanting_a_room_feature(None);

    assert_eq!(
        problem.preferences.cost(ONLY, MONDAY, &["lab".to_string()]),
        0.0,
        "the wanted feature is present: nothing charged"
    );
    assert_eq!(
        problem.preferences.cost(ONLY, MONDAY, &[]),
        PREFERENCE_WEIGHT,
        "the wanted feature is absent: the full unmet cost, multiplier 1.0"
    );
}

#[test]
fn a_lecturer_stating_only_a_room_preference_is_still_counted() {
    // The regression this fixture exists for: `one_lecturer_wanting_a_room_feature`
    // states NO day or block at all, so `narrow()` returns `None` for this
    // Person and `counted` (the day/block set) is empty. If room preference
    // rode on `counted`, this would report 0 in EVERY room — the exact bug
    // `room_wanted`'s independent gate exists to avoid.
    let problem = testing::one_lecturer_wanting_a_room_feature(None);
    assert!(
        problem.preferences.cost(ONLY, MONDAY, &[]) > 0.0,
        "a room-only preference must still be charged when unmet, even though \
         `counted` (day/block) is empty for this lecturer"
    );
}

#[test]
fn day_block_and_room_are_independent_additive_terms() {
    // One lecturer wanting Monday AND "lab", multiplier 1.0. Four
    // combinations, and the two axes must not interact: changing the room
    // must never move the day/block component and vice versa.
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![
            calendry_solver_core::problem::Room {
                features: vec!["lab".to_string()],
                ..testing::room("R0")
            },
            testing::room("R1"),
        ],
        persons: vec![testing::person_with_preference(
            "wants-monday-lab",
            &[],
            calendry_solver_core::preferences::Preference {
                days: vec![1],
                blocks: vec![],
                room_features: vec!["lab".to_string()],
                weight_multiplier: None,
            },
        )],
        offerings: vec![testing::with_lecturers(
            testing::offering("S", 1, &[0, 1]),
            &[0],
        )],
        constraints: testing::with_preference(vec![testing::preference_rule(
            "c-pref",
            PREFERENCE_WEIGHT,
        )]),
        ..ProblemSpec::new(testing::two_day_grid())
    });
    let w = PREFERENCE_WEIGHT;
    let lab = ["lab".to_string()];
    let no_lab: [String; 0] = [];

    // Monday (day met) + lab (room met): both terms at 0.
    assert_eq!(problem.preferences.cost(ONLY, MONDAY, &lab), 0.0);
    // Monday (day met) + no lab (room unmet): only the room term charges.
    assert_eq!(problem.preferences.cost(ONLY, MONDAY, &no_lab), w, "room term alone");
    // Saturday (day unmet) + lab (room met): only the day term charges.
    assert_eq!(problem.preferences.cost(ONLY, SATURDAY, &lab), w, "day term alone");
    // Saturday (day unmet) + no lab (room unmet): both terms charge, summed.
    assert_eq!(problem.preferences.cost(ONLY, SATURDAY, &no_lab), 2.0 * w, "both terms");
}

// ---------------------------------------------------------------------------
// Incremental maintenance
// ---------------------------------------------------------------------------

#[test]
fn incremental_objective_matches_full_recomputation() {
    // The preference cost is accumulated as a delta by `place`/`unplace` and
    // recomputed from scratch by `recompute_objective`. Debug builds assert the
    // two agree on every LNS iteration; this pins the end state across many
    // instances and several budgets, so the property holds wherever a run
    // happens to stop.
    //
    // `seeded_preference_instance` exists because the existing seeded instances
    // carry no preference at all — the drift assertion would have covered this
    // term by construction and tested nothing.
    for seed in 0..12u64 {
        let problem = testing::seeded_preference_instance(seed);
        assert!(
            !problem.preferences.is_empty(),
            "seed {seed}: the fixture must actually configure the rule"
        );

        for max_moves in [50u64, 500, 5_000, 50_000] {
            let outcome = solve(&problem, SEED ^ seed, moves(max_moves), &NeverHalt);
            let full = recompute_objective(&problem, &outcome.solution);
            assert!(
                objectives_agree(outcome.objective, full),
                "seed {seed} budget {max_moves}: objective drifted, \
                 incremental {:?} vs recomputed {:?}",
                outcome.objective,
                full
            );
        }
    }
}

#[test]
fn a_placement_and_its_removal_cancel_exactly() {
    // The narrow version of the same property, which localises a failure: if
    // `place` and `unplace` read the table differently the residue shows up
    // here without a search in the way.
    let problem = testing::two_lecturers_with_opposing_preferences();
    let mut trial = calendry_solver_core::Trial::construct(&problem);

    let before = trial.objective().soft;
    let at = trial.solution().get(ONLY).expect("constructed");
    let removed = trial.unplace(ONLY).expect("was placed");
    assert_eq!(trial.objective().soft, 0.0, "the only placement's cost is the whole soft total");
    assert!(trial.place(ONLY, removed));
    assert_eq!(trial.objective().soft, before, "place/unplace must cancel");
    assert_eq!(at.start, removed.start);
}

// ---------------------------------------------------------------------------
// The breakdown a human is shown
// ---------------------------------------------------------------------------

#[test]
fn the_breakdown_reports_the_unmet_fraction_it_charged_for() {
    // The breakdown is what the app shows a person to explain the score, so it
    // must contain the number the objective actually charged — the same
    // requirement that made `MinimizeRoomRank` accumulate its graded severity
    // rather than multiplying a count by a weight.
    let problem = testing::two_lecturers_with_opposing_preferences();
    let solution = placed_at(&problem, MONDAY);

    let components = soft_breakdown(&problem, &solution);
    let pref = components
        .iter()
        .find(|c| c.constraint_type == "PersonPreferenceFit")
        .expect("one component per configured instance");

    assert_eq!(pref.constraint_id, "c-pref");
    // Monday costs 1.0 * weight, and one placed Session missed something.
    assert_eq!(pref.weighted, 1.0 * PREFERENCE_WEIGHT);
    assert_eq!(pref.raw_count, 1, "Sessions that missed something a lecturer asked for");

    // A fully-satisfied placement reports nothing rather than reporting a
    // success — the count is breaches, like every other soft component's.
    let satisfied = soft_breakdown(&problem, &placed_at(&problem, SATURDAY));
    let pref = satisfied
        .iter()
        .find(|c| c.constraint_type == "PersonPreferenceFit")
        .expect("still reported when it costs less");
    assert_eq!(pref.weighted, 0.25 * PREFERENCE_WEIGHT);
    assert_eq!(pref.raw_count, 1, "0.25 unmet is still a breach");
}

#[test]
fn the_model_is_inert_without_a_grid_to_price_against() {
    // Degenerate but reachable through `PreferenceModel::build`'s own
    // signature, and the read path must not index into an empty table.
    let model = PreferenceModel::default();
    assert_eq!(model.cost(ONLY, MONDAY, &[]), 0.0);
    assert_eq!(model.unmet(ONLY, MONDAY, &[]), 0.0);
    assert!(model.is_empty());
    assert_eq!(model.max_cost_per_placement(), 0.0);
}

// ---------------------------------------------------------------------------
// Fixtures local to this file
// ---------------------------------------------------------------------------

fn one_session_with_lecturers(persons: Vec<calendry_solver_core::problem::Person>) -> Problem {
    one_session_on_grid(testing::two_day_grid(), persons)
}

fn one_session_on_grid(
    slots: calendry_solver_core::slots::SlotTable,
    persons: Vec<calendry_solver_core::problem::Person>,
) -> Problem {
    let rule = vec![testing::preference_rule("c-pref", PREFERENCE_WEIGHT)];
    build(slots, persons, rule)
}

fn one_session_with_lecturers_and_rule(
    persons: Vec<calendry_solver_core::problem::Person>,
    rule: Vec<calendry_solver_core::PreferenceInstance>,
) -> Problem {
    build(testing::two_day_grid(), persons, rule)
}

fn build(
    slots: calendry_solver_core::slots::SlotTable,
    persons: Vec<calendry_solver_core::problem::Person>,
    rule: Vec<calendry_solver_core::PreferenceInstance>,
) -> Problem {
    let lecturers: Vec<u32> = (0..persons.len() as u32).collect();
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons,
        offerings: vec![testing::with_lecturers(
            testing::offering("S", 1, &[0]),
            &lecturers,
        )],
        constraints: testing::with_preference(rule),
        ..ProblemSpec::new(slots)
    })
}

/// The one placement pinned at `slot`, so a breakdown can be read off a known
/// assignment rather than off whatever the search chose.
fn placed_at(problem: &Problem, slot: SlotIdx) -> Solution {
    let mut solution = Solution::empty(problem);
    solution.set(
        ONLY,
        Some(calendry_solver_core::Placement {
            start: slot,
            room: calendry_solver_core::ids::RoomIdx(0),
        }),
    );
    solution
}
