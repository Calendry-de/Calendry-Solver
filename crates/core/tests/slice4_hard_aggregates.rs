//! Slice 4 acceptance tests: the three remaining hard types.
//!
//! These have three genuinely different shapes, and the tests are built to
//! prove each behaves according to its own shape rather than by accident:
//!
//! * `LecturerVeto` is **unary** — a slot mask, like the soft costs.
//! * `OnlineOnsiteSameDay` is a **day-granularity filter** the search can never
//!   violate.
//! * `MaxOnlineShare` is an **aggregate ratio** carried on the objective, so it
//!   *can* survive into a returned solution and be reported.
//!
//! The highest-value test here is `aggregate_counters_match_full_recomputation`
//! — the counterpart to slice 3's delta-drift test. The share counters have a
//! *moving denominator*, which is strictly more error-prone than the soft sums
//! were, so it is asserted per iteration in debug builds as well.

use calendry_solver_core::aggregates::ShareWindow;
use calendry_solver_core::constraints::{
    LECTURER_VETO, MAX_ONLINE_SHARE, ONLINE_ONSITE_SAME_DAY, evaluate_hard,
};
use calendry_solver_core::ids::PlacementIdx;
use calendry_solver_core::search::{Budget, NeverHalt, recompute_objective, solve};
use calendry_solver_core::{Problem, testing};

const SEED: u64 = 0xC0FFEE;

fn budget() -> Budget {
    Budget { max_wall_millis: 0, max_moves: 50_000 }
}

fn run(problem: &Problem) -> calendry_solver_core::SolveOutcome {
    solve(problem, SEED, budget(), &NeverHalt)
}

/// How many placed Sessions sit in a virtual room.
fn online_count(problem: &Problem, outcome: &calendry_solver_core::SolveOutcome) -> usize {
    problem
        .placement_ids()
        .filter_map(|p| outcome.solution.get(p))
        .filter(|pl| problem.rooms[pl.room.get()].is_virtual)
        .count()
}

// ---------------------------------------------------------------------------
// LecturerVeto — unary
// ---------------------------------------------------------------------------

#[test]
fn a_lecturer_is_not_scheduled_during_their_blackout() {
    // Two blocks, one room. The lecturer is blacked out on block 0, which is
    // exactly where greedy would otherwise put the Session.
    let problem = testing::lecturer_blacked_out_on_first_block(testing::all_constraints());
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(
        outcome.solution.get(PlacementIdx(0)).unwrap().start.0,
        1,
        "the blacked-out block must be avoided"
    );
}

#[test]
fn without_the_veto_the_same_instance_takes_the_blacked_out_slot() {
    // Falsification: with the rule disabled the blackout is inert and greedy
    // takes block 0. So the test above is measuring the rule, not the grid.
    let problem = testing::lecturer_blacked_out_on_first_block(testing::without_lecturer_veto());
    let outcome = run(&problem);
    assert_eq!(outcome.solution.get(PlacementIdx(0)).unwrap().start.0, 0);
}

#[test]
fn a_blackout_violation_present_in_the_input_is_reported() {
    // Force the clash: the lecturer is unavailable on every block, so no
    // placement can satisfy the veto and the search must leave it unplaced
    // rather than quietly scheduling into a blackout.
    let problem = testing::assemble(
        testing::grid(2, 1),
        testing::rooms(1),
        vec![],
        vec![testing::person_with_blackouts(
            "always-out",
            &[],
            vec![testing::blackout(&[], &[], &[])],
        )],
        vec![testing::with_lecturers(testing::offering("S", 1, &[0]), &[0])],
        vec![],
        testing::all_constraints(),
    );
    let outcome = run(&problem);

    assert_eq!(outcome.solution.placed_count(), 0, "nothing is placeable");
    assert!(
        !outcome
            .hard_violations
            .iter()
            .any(|v| v.constraint_type == LECTURER_VETO),
        "an unplaced Session must not also be reported as a veto breach"
    );
    assert!(
        outcome.objective.unplaced > 0,
        "the shortfall must surface on the objective"
    );
}

// ---------------------------------------------------------------------------
// OnlineOnsiteSameDay — day-granularity, SOFT since the reclassification
// ---------------------------------------------------------------------------
//
// These four tests changed direction rather than being deleted, and the reason
// is the point of the change: the rule used to eliminate candidate placements
// inside `is_free`, so the search COULD NOT produce a mixed day. It is now
// priced on the objective, so it can — and must, when every alternative costs
// more. What is still guaranteed is that a mix is never FREE.

#[test]
fn a_group_prefers_not_to_mix_online_and_onsite_on_one_day() {
    // Was `a_group_does_not_mix…`, asserting an all-or-nothing day. The
    // preference still wins here, because this instance has an alternative that
    // costs nothing — which is exactly the case where soft and hard agree.
    let problem = testing::group_day_with_both_room_types(testing::all_constraints());
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(outcome.solution.placed_count(), 2);

    let online = online_count(&problem, &outcome);
    assert!(
        online == 0 || online == 2,
        "with a free alternative the day should still come out unmixed, got {online} of 2 online"
    );
    assert_eq!(
        outcome.objective.day_mix_cost, 0.0,
        "an unmixed day must cost nothing"
    );
}

#[test]
fn a_mixed_day_costs_the_configured_weight() {
    // The falsification that replaces `without_the_rule_the_same_instance_does_mix`.
    //
    // That test proved the rule was doing something by removing it and watching
    // the mix appear. Removing a SOFT rule changes the price rather than the
    // feasibility, so the sharper question is what a mix costs — and the answer
    // has to be the configured weight per mixed cell, or the objective is not
    // actually carrying the rule.
    let problem = testing::group_day_with_both_room_types(testing::all_constraints());
    let unpriced = testing::group_day_with_both_room_types(testing::without_day_mix());

    assert_eq!(
        unpriced.day_mix_weight, 0.0,
        "with no instance configured a mixed day is free"
    );
    assert_eq!(
        problem.day_mix_weight, 5.0,
        "the fixture configures the catalogue's default weight"
    );

    // Force the mix rather than hoping for it: one Session pinned online, the
    // other with only an on-site room left.
    let mixed = testing::solution_mixing_one_day(&problem);
    let objective = calendry_solver_core::search::recompute_objective(&problem, &mixed);

    assert_eq!(
        objective.day_mix_cost, 5.0,
        "one mixed (group, day) cell must cost exactly one weight"
    );
    assert!(
        objective.total(problem.hard_penalty) > 0.0,
        "and it must reach the scalar objective the search minimises"
    );
}

#[test]
fn a_mixed_day_is_no_longer_a_hard_violation() {
    // Was `a_mixed_day_already_in_immovable_input_is_reported`. The input is
    // identical; the expectation is inverted.
    //
    // It used to be reportable ONLY from immovable input, because the filter
    // made it unreachable for the search — which is what made it a defect worth
    // naming. Now the search creates mixed days deliberately, so reporting them
    // as hard violations would report the objective working as a fault. They
    // travel in the objective breakdown instead.
    let mut online_fixed = testing::fixed_for_groups("pinned-online", 0, 0, &[0]);
    online_fixed.kind = "lecture".to_string();
    let mut onsite_fixed = testing::fixed_for_groups("pinned-onsite", 1, 1, &[0]);
    onsite_fixed.kind = "lecture".to_string();

    let problem = testing::assemble(
        testing::grid(2, 1),
        testing::online_first_rooms(),
        vec![testing::group("G", None)],
        vec![],
        vec![],
        vec![online_fixed, onsite_fixed],
        testing::all_constraints(),
    );

    let empty = calendry_solver_core::Solution::empty(&problem);
    let violations = evaluate_hard(&problem, &empty);

    assert!(
        !violations
            .iter()
            .any(|v| v.constraint_type == ONLINE_ONSITE_SAME_DAY),
        "a mixed day is soft and must not be reported as a hard violation, got {violations:?}"
    );

    // ...but it is not silently dropped either. The count and its cost are in
    // the breakdown, which is what the app shows a human to explain the score.
    let breakdown = calendry_solver_core::search::soft_breakdown(&problem, &empty);
    let component = breakdown
        .iter()
        .find(|c| c.constraint_type == ONLINE_ONSITE_SAME_DAY)
        .expect("the day-mix component must appear in the breakdown");

    assert_eq!(component.raw_count, 1, "one mixed (group, day) cell");
    assert_eq!(component.weighted, 5.0);
}

// ---------------------------------------------------------------------------
// MaxOnlineShare — aggregate ratio
// ---------------------------------------------------------------------------

#[test]
fn the_online_share_cap_is_respected() {
    // 4 Sessions, cap 0.25 => floor(0.25 * 4) = 1 may be online.
    let problem = testing::share_capped_group(vec![testing::share_rule(
        "cap",
        0.25,
        ShareWindow::PerTerm,
    )]);
    let outcome = run(&problem);

    assert_eq!(outcome.solution.placed_count(), 4);
    assert!(
        online_count(&problem, &outcome) <= 1,
        "cap allows 1 of 4 online, got {}",
        online_count(&problem, &outcome)
    );
    assert_eq!(outcome.objective.aggregate, 0, "no cell should be over");
}

#[test]
fn without_the_cap_more_sessions_go_online() {
    // Falsification: unconstrained, greedy fills the virtual room first, so
    // strictly more than the cap would allow ends up online.
    let problem = testing::share_capped_group(vec![]);
    let outcome = run(&problem);
    assert!(
        online_count(&problem, &outcome) > 1,
        "unconstrained this instance puts more than one online"
    );
}

#[test]
fn per_week_and_per_term_windows_differ() {
    // 4 Sessions over 2 weeks, cap 0.5.
    //   PER_TERM: 2 of 4 online is allowed.
    //   PER_WEEK: at most 1 of the 2 in each week.
    let term = run(&testing::share_across_two_weeks(vec![testing::share_rule(
        "cap",
        0.5,
        ShareWindow::PerTerm,
    )]));
    let week = run(&testing::share_across_two_weeks(vec![testing::share_rule(
        "cap",
        0.5,
        ShareWindow::PerWeek,
    )]));

    assert_eq!(term.objective.aggregate, 0);
    assert_eq!(week.objective.aggregate, 0);

    // The window is genuinely read: the per-week run must not concentrate its
    // online Sessions into a single week.
    let problem = testing::share_across_two_weeks(vec![testing::share_rule(
        "cap",
        0.5,
        ShareWindow::PerWeek,
    )]);
    let mut online_per_week = [0u32; 2];
    for p in problem.placement_ids() {
        if let Some(pl) = week.solution.get(p)
            && problem.rooms[pl.room.get()].is_virtual
        {
            online_per_week[problem.slots.flags(pl.start).week as usize] += 1;
        }
    }
    assert!(
        online_per_week.iter().all(|&n| n <= 1),
        "per-week cap allows 1 online per week, got {online_per_week:?}"
    );
}

#[test]
fn an_unsatisfiable_cap_is_reported_rather_than_silently_dropped() {
    // Only a virtual room exists, so every Session must be online, but the cap
    // is zero. MaxOnlineShare lives on the objective rather than acting as a
    // filter, so the run succeeds and REPORTS the breach — the same shape as an
    // unplaced Session, not a new exception.
    let problem = testing::assemble(
        testing::grid(2, 1),
        vec![testing::room_with("V", 1, true)],
        vec![testing::group("G", None)],
        vec![],
        vec![testing::with_groups(testing::offering("S", 2, &[0]), &[0])],
        vec![],
        calendry_solver_core::ConstraintSet {
            max_online_share: vec![testing::share_rule("cap", 0.0, ShareWindow::PerTerm)],
            ..testing::without_day_mix()
        },
    );
    let outcome = run(&problem);

    assert_eq!(outcome.solution.placed_count(), 2, "placement still happens");
    assert!(outcome.objective.aggregate > 0, "the breach must reach the objective");
    assert!(
        outcome
            .hard_violations
            .iter()
            .any(|v| v.constraint_type == MAX_ONLINE_SHARE),
        "and must be reported, got {:?}",
        outcome.hard_violations
    );
}

// ---------------------------------------------------------------------------
// The aggregate-counter drift test — highest value in this slice
// ---------------------------------------------------------------------------

#[test]
fn aggregate_counters_match_full_recomputation() {
    // The share counters are maintained incrementally across ruin and repair,
    // and unlike the soft sums their DENOMINATOR moves: relocating a Session
    // between weeks changes one numerator and two denominators. If those
    // counters drift, the search optimizes a constraint that no longer
    // describes the schedule, and every other test in this file still passes.
    //
    // Debug builds assert the whole objective — including this term — on every
    // LNS iteration; this pins the end state across many instances and budgets.
    for seed in 0..12u64 {
        let problem = testing::seeded_aggregate_instance(seed);

        for max_moves in [50u64, 500, 5_000, 50_000] {
            let outcome = solve(
                &problem,
                SEED ^ seed,
                Budget { max_wall_millis: 0, max_moves },
                &NeverHalt,
            );
            let full = recompute_objective(&problem, &outcome.solution);

            assert_eq!(
                outcome.objective.aggregate, full.aggregate,
                "seed {seed} budget {max_moves}: share violations drifted \
                 (incremental {} vs recomputed {})",
                outcome.objective.aggregate, full.aggregate
            );
            assert_eq!(
                outcome.objective.unplaced, full.unplaced,
                "seed {seed} budget {max_moves}: unplaced drifted"
            );
            assert!(
                (outcome.objective.soft - full.soft).abs() <= 1e-9 * (1.0 + full.soft.abs()),
                "seed {seed} budget {max_moves}: soft drifted"
            );
        }
    }
}

#[test]
fn aggregate_instances_stay_deterministic() {
    for seed in 0..6u64 {
        let problem = testing::seeded_aggregate_instance(seed);
        let first = solve(&problem, SEED, budget(), &NeverHalt);
        let again = solve(&problem, SEED, budget(), &NeverHalt);

        let a: Vec<_> = problem.placement_ids().map(|p| first.solution.get(p)).collect();
        let b: Vec<_> = problem.placement_ids().map(|p| again.solution.get(p)).collect();
        assert_eq!(a, b, "seed {seed}");
        assert_eq!(first.objective, again.objective);
        assert_eq!(first.hard_violations, again.hard_violations);
    }
}

#[test]
fn the_search_never_creates_a_veto_breach_and_never_hard_reports_a_day_mix() {
    /*
     * THE ASSERTION DIRECTION FLIPPED FOR ONE OF THE TWO, AND ONLY ONE.
     *
     * `LecturerVeto` is still a filter, so the old claim stands unchanged: the
     * search cannot produce one.
     *
     * `OnlineOnsiteSameDay` is no longer a filter, so "the search never creates
     * a day mix" is no longer true and must not be asserted — it would pin the
     * behaviour the reclassification exists to remove. What replaces it is the
     * property that survived: a mixed day never appears among the HARD
     * violations, whether the search made it or the caller pinned it.
     *
     * Keeping the loop over twelve seeds matters more now, not less. A filter
     * either holds or does not; a priced term can be right on average and wrong
     * on a particular instance, so the sweep is the part doing the work.
     */
    for seed in 0..12u64 {
        let problem = testing::seeded_aggregate_instance(seed);
        let outcome = run(&problem);
        for v in &outcome.hard_violations {
            assert_ne!(
                v.constraint_type, ONLINE_ONSITE_SAME_DAY,
                "seed {seed}: a soft rule must never surface as a hard violation"
            );
            assert_ne!(
                v.constraint_type, LECTURER_VETO,
                "seed {seed}: LecturerVeto is still a filter"
            );
        }
        assert!(
            outcome.objective.day_mix_cost >= 0.0,
            "seed {seed}: the day-mix term must be a real, non-negative cost"
        );
    }
}
