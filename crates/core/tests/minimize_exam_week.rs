//! `MinimizeExamWeek`, once an exam week can belong to some cohorts and not
//! others (ADR-0033).
//!
//! The type used to be a `SoftParams` variant, priced out of the
//! `(kind-profile, slot, room)` table. It is not one any more: an exam week may
//! be scoped to Groups, and two Offerings of one kind — one profile, one table
//! row — routinely serve different cohorts. So the cost now reads the Offering,
//! through a mask precomputed in `Problem::build`.
//!
//! That makes three separate things worth pinning, and they fail in different
//! ways:
//!
//! 1. **The refactor moved no number.** With nothing scoped, the mask is
//!    exactly "every slot whose week is an exam week", so every charge must be
//!    what the table charged, to the last place — including `hard_penalty`,
//!    which used to receive this type's weight through `soft.total_weight` and
//!    now needs its own term. A shrunken `hard_penalty` is invisible until a
//!    soft preference outranks a hole in the timetable.
//! 2. **The direction the scope travels through the hierarchy.** Three
//!    plausible expansions exist and two are wrong; on a flat hierarchy all
//!    three agree, so the guard is a mirrored pair over one two-level fixture,
//!    exactly as in `group_veto.rs`. Under `invert` a wrong expansion does not
//!    merely over-charge — it *steers*, actively pulling a cohort's lecture
//!    into a week it must avoid, which is the argument ADR-0033 adds beyond
//!    ADR-0027.
//! 3. **What `invert` means when the mask is not universal.** One mask, two
//!    sides, and a tenant may enable both directions at once.

use calendry_solver_core::ids::PlacementIdx;
use calendry_solver_core::problem::{MinimizeExamWeekInstance, ProblemSpec};
use calendry_solver_core::search::{objectives_agree, recompute_objective, soft_breakdown};
use calendry_solver_core::slots::{SlotTable, WeekKind, WeekSpec};
use calendry_solver_core::testing::{self, exam_scope, group, with_exam_week};
use calendry_solver_core::{Problem, Solution};

mod common;
use calendry_solver_core::search::{NeverHalt, solve};
use common::{SEED, moves};

const WEIGHT: f64 = 7.0;

/// One block a day, one active day, so a slot index IS a week index — the
/// smallest grid on which "this cohort's exam week" and "that cohort's" are
/// different statements.
fn weeks_grid(kinds: &[WeekKind]) -> SlotTable {
    SlotTable::build(
        1,
        &[1],
        &kinds
            .iter()
            .map(|&kind| WeekSpec { kind, holiday_weekdays: vec![] })
            .collect::<Vec<_>>(),
    )
    .expect("grid")
}

/// Cohort `C` (index 0) with one child seminar `S` (index 1) — the two-level
/// fixture the direction pair needs. One Offering per named Group, each needing
/// one Session, both eligible for the single Room.
fn two_level(
    kinds: &[WeekKind],
    scopes: Vec<calendry_solver_core::problem::ExamWeekScope>,
    attached_to: u32,
    invert: bool,
) -> Problem {
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![group("C", None), group("S", Some(0))],
        offerings: vec![testing::with_groups(
            testing::offering("O", 1, &[0]),
            &[attached_to],
        )],
        constraints: with_exam_week(testing::all_constraints(), WEIGHT, invert),
        exam_week_groups: scopes,
        ..ProblemSpec::new(weeks_grid(kinds))
    })
}

/// What the objective charges for placing the only Offering at `slot`.
fn cost_at(problem: &Problem, slot: u32) -> f64 {
    let o = &problem.offerings[0];
    problem.exam_week_cost(o, calendry_solver_core::ids::SlotIdx(slot))
}

fn solved(problem: &Problem, budget: u64) -> (Solution, f64) {
    let outcome = solve(problem, SEED, moves(budget), &NeverHalt);
    (outcome.solution, outcome.objective.soft)
}

fn slot_of(s: &Solution, i: u32) -> u32 {
    s.get(PlacementIdx(i)).expect("placed").start.0
}

// ---------------------------------------------------------------------------
// Compatibility: the refactor moved no number
// ---------------------------------------------------------------------------

#[test]
fn an_unscoped_exam_week_charges_exactly_what_the_soft_table_charged() {
    // Week 0 exam, week 1 teaching, nothing scoped. The mask is the global
    // exam-week set, so the charge is the flat weight inside it and zero
    // outside — which is precisely what a `(profile, slot, room)` table row
    // summing this instance's weight contained.
    let problem = two_level(&[WeekKind::Exam, WeekKind::Teaching], vec![], 0, false);

    assert_eq!(cost_at(&problem, 0), WEIGHT, "inside an unscoped exam week");
    assert_eq!(cost_at(&problem, 1), 0.0, "outside it");
}

#[test]
fn hard_penalty_is_unchanged_when_the_type_leaves_the_soft_table() {
    // THE SILENT-SHRINK GUARD. `MinimizeExamWeek` used to reach `hard_penalty`
    // through `soft.total_weight`; moving it out removed that contribution, so
    // `Problem::build` grew a replacement term. Without it the bound shrinks by
    // exactly this delta and a soft preference gains ground on an unplaced
    // Session — a defect no correctness test would notice.
    let kinds = [WeekKind::Exam, WeekKind::Teaching];
    let without = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 3, &[0])],
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(weeks_grid(&kinds))
    });
    let with = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 3, &[0])],
        constraints: with_exam_week(testing::all_constraints(), WEIGHT, false),
        ..ProblemSpec::new(weeks_grid(&kinds))
    });

    let placements = 3.0;
    assert_eq!(
        with.hard_penalty - without.hard_penalty,
        WEIGHT * placements,
        "the ceiling must grow by weight x placements, the same shape every \
         other per-placement soft term contributes"
    );
}

#[test]
fn a_scoped_exam_week_leaves_another_cohorts_offering_unpriced() {
    // The feature, in one assertion: week 0 is an exam week for `C` only, so
    // the Offering attached to the unrelated sibling pays nothing there.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![group("C", None), group("D", None)],
        offerings: vec![
            testing::with_groups(testing::offering("for-C", 1, &[0]), &[0]),
            testing::with_groups(testing::offering("for-D", 1, &[0]), &[1]),
        ],
        constraints: with_exam_week(testing::all_constraints(), WEIGHT, false),
        exam_week_groups: vec![exam_scope(0, &[0])],
        ..ProblemSpec::new(weeks_grid(&[WeekKind::Exam, WeekKind::Teaching]))
    });

    let (c, d) = (&problem.offerings[0], &problem.offerings[1]);
    let at0 = calendry_solver_core::ids::SlotIdx(0);
    assert_eq!(problem.exam_week_cost(c, at0), WEIGHT, "week 0 is C's exam week");
    assert_eq!(problem.exam_week_cost(d, at0), 0.0, "and is nobody else's");
}

// ---------------------------------------------------------------------------
// Group direction — the mirrored pair over ONE two-level fixture
// ---------------------------------------------------------------------------

#[test]
fn a_parents_exam_period_binds_its_child() {
    // Scope on the cohort, Session on the seminar beneath it. Passes under
    // `expand_ancestry` AND under `expand_conflict`, and fails under
    // `expand_subtree` — it exists so its mirror below is not vacuously green,
    // the same role `a_parents_absence_binds_its_child` plays in `group_veto.rs`.
    let problem =
        two_level(&[WeekKind::Exam, WeekKind::Teaching], vec![exam_scope(0, &[0])], 1, false);

    assert_eq!(
        cost_at(&problem, 0),
        WEIGHT,
        "a programme's exam fortnight covers its cohorts, so the query walks UP"
    );
}

#[test]
fn a_childs_exam_period_does_not_bind_its_parent() {
    // The mirror, and the one that discriminates: passes under
    // `expand_ancestry` ONLY. Deleting it leaves a fully green suite over a
    // rule pointing the wrong way — one seminar's exam period would redefine
    // the exam period of the lecture its entire cohort attends.
    let problem =
        two_level(&[WeekKind::Exam, WeekKind::Teaching], vec![exam_scope(0, &[1])], 0, false);

    assert_eq!(cost_at(&problem, 0), 0.0, "a seminar's exam period is not its cohort's");
}

#[test]
fn an_inverted_rule_never_pulls_a_cohort_lecture_into_a_seminars_exam_week() {
    // What ADR-0033 adds beyond ADR-0027. For `GroupVeto` a wrong expansion
    // over-blocks, which is at least conservative. Here, under `invert`, the
    // rule SEEKS the mask — so `expand_conflict` would hand the cohort's
    // lecture the seminar's exam period as its own and the search would
    // actively MOVE it there. The wrong expansion stops being over-cautious
    // and starts steering.
    //
    // The grid is teaching-then-exam, for the reason
    // `testing::teaching_then_exam_grid` exists: greedy's "earliest slot"
    // default already sits outside the exam week, so a wrong expansion has to
    // make a move to fail this, rather than merely failing to make one. On the
    // reverse grid both implementations leave the Session at slot 0 and the
    // test would be green against the bug.
    let problem =
        two_level(&[WeekKind::Teaching, WeekKind::Exam], vec![exam_scope(1, &[1])], 0, true);

    // The mechanism, stated before the consequence: the cohort's Offering has
    // no exam period of its own, so an inverted rule charges it uniformly and
    // has nothing to steer with. `expand_conflict` would make slot 1 free and
    // slot 0 charged.
    assert_eq!(cost_at(&problem, 0), WEIGHT, "no exam period of its own");
    assert_eq!(cost_at(&problem, 1), WEIGHT, "so both slots cost the same");

    let (solution, _) = solved(&problem, 5_000);
    assert_eq!(
        slot_of(&solution, 0),
        0,
        "with nothing to gain the search must stay put — a seminar's exam \
         period must not attract the lecture its whole cohort attends"
    );
}

// ---------------------------------------------------------------------------
// `invert` semantics
// ---------------------------------------------------------------------------

#[test]
fn an_inverted_rule_seeks_this_offerings_own_exam_week_not_another_cohorts() {
    // The sharpest single test of the feature. Week 0 is A's exam period,
    // week 1 is B's, week 2 is ordinary teaching. Each cohort's exam Offering
    // must find ITS OWN week.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![group("A", None), group("B", None)],
        offerings: vec![
            testing::with_groups(testing::offering("exam-A", 1, &[0]), &[0]),
            testing::with_groups(testing::offering("exam-B", 1, &[0]), &[1]),
        ],
        constraints: with_exam_week(testing::all_constraints(), WEIGHT, true),
        exam_week_groups: vec![exam_scope(0, &[0]), exam_scope(1, &[1])],
        ..ProblemSpec::new(weeks_grid(&[WeekKind::Exam, WeekKind::Exam, WeekKind::Teaching]))
    });

    let (solution, _) = solved(&problem, 5_000);
    assert_eq!(slot_of(&solution, 0), 0, "A's exam sits in A's exam period");
    assert_eq!(slot_of(&solution, 1), 1, "B's exam sits in B's");
}

#[test]
fn an_offering_in_two_cohorts_treats_the_union_of_their_exam_periods_as_its_own() {
    // Union, not intersection, and the inverted direction is what proves it
    // matters: the intersection here is EMPTY, so an inverted rule would charge
    // a joint Offering at every slot and steer nothing at all.
    let kinds = [WeekKind::Exam, WeekKind::Exam, WeekKind::Teaching];
    let scopes = || vec![exam_scope(0, &[0]), exam_scope(1, &[1])];
    let build = |invert: bool| {
        testing::assemble(ProblemSpec {
            rooms: testing::rooms(1),
            groups: vec![group("A", None), group("B", None)],
            offerings: vec![testing::with_groups(
                testing::offering("joint", 1, &[0]),
                &[0, 1],
            )],
            constraints: with_exam_week(testing::all_constraints(), WEIGHT, invert),
            exam_week_groups: scopes(),
            ..ProblemSpec::new(weeks_grid(&kinds))
        })
    };

    let plain = build(false);
    assert_eq!(cost_at(&plain, 0), WEIGHT, "collides with A's exams");
    assert_eq!(cost_at(&plain, 1), WEIGHT, "and with B's");
    assert_eq!(cost_at(&plain, 2), 0.0, "the teaching week is free");

    let inverted = build(true);
    assert_eq!(cost_at(&inverted, 0), 0.0, "a joint exam may sit in either period");
    assert_eq!(cost_at(&inverted, 1), 0.0);
    assert_eq!(cost_at(&inverted, 2), WEIGHT, "and is charged outside both");
}

#[test]
fn an_offering_with_no_groups_matches_no_scoped_exam_week() {
    // `expand_ancestry(&[])` is empty, so a group-less Offering — an all-staff
    // meeting — sits in nobody's scoped exam period. But a term-global one is
    // still term-global, which is the fail-open convention reaching all the way
    // down.
    let kinds = [WeekKind::Exam, WeekKind::Teaching];
    let build = |scopes: Vec<calendry_solver_core::problem::ExamWeekScope>| {
        testing::assemble(ProblemSpec {
            rooms: testing::rooms(1),
            groups: vec![group("A", None)],
            offerings: vec![testing::offering("staff-meeting", 1, &[0])],
            constraints: with_exam_week(testing::all_constraints(), WEIGHT, false),
            exam_week_groups: scopes,
            ..ProblemSpec::new(weeks_grid(&kinds))
        })
    };

    assert_eq!(cost_at(&build(vec![exam_scope(0, &[0])]), 0), 0.0, "scoped: not mine");
    assert_eq!(cost_at(&build(vec![]), 0), WEIGHT, "unscoped: everybody's");
    assert_eq!(cost_at(&build(vec![exam_scope(0, &[])]), 0), WEIGHT, "explicitly everybody's");
}

#[test]
fn an_inverted_rule_over_a_group_less_offering_is_a_constant_and_is_not_special_cased() {
    // DELIBERATE, and the test that fails if someone copies
    // `charged_specialized_rooms_for`'s empty-mask shortcut. This Offering has
    // no exam period, so an inverted rule charges it EVERYWHERE — a
    // non-steering constant, and the honest reading: it is the same thing an
    // inverted rule over a calendar with no exam weeks at all already charged.
    //
    // Special-casing it to zero would make one Offering's cost depend on
    // whether some OTHER week carries a scope list, which no other term in the
    // objective does.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![group("A", None)],
        offerings: vec![testing::offering("staff-meeting", 1, &[0])],
        constraints: with_exam_week(testing::all_constraints(), WEIGHT, true),
        exam_week_groups: vec![exam_scope(0, &[0])],
        ..ProblemSpec::new(weeks_grid(&[WeekKind::Exam, WeekKind::Teaching]))
    });

    assert_eq!(cost_at(&problem, 0), WEIGHT, "outside its (empty) exam period");
    assert_eq!(cost_at(&problem, 1), WEIGHT, "and outside it here too");

    let (solution, soft) = solved(&problem, 2_000);
    assert_eq!(soft, WEIGHT, "one placement, charged once, wherever it went");
    let component = soft_breakdown(&problem, &solution)
        .into_iter()
        .find(|c| c.constraint_type == "MinimizeExamWeek")
        .expect("reported");
    assert_eq!(component.raw_count, 1, "visible as a count equal to the placement count");
}

#[test]
fn both_directions_of_one_axis_are_charged_separately() {
    // ADR-0024's separately-instantiable hazard: `invert` makes this one type
    // rather than two, but it does not stop a tenant enabling both directions.
    // Every placement then pays exactly one of the two — which is also why the
    // Offering carries two charges rather than one, and why `hard_penalty`'s
    // per-placement ceiling is still the summed weight.
    let mut constraints = testing::all_constraints();
    constraints.minimize_exam_week = vec![
        MinimizeExamWeekInstance { id: "avoid".into(), kinds: vec![], weight: 3.0, invert: false },
        MinimizeExamWeekInstance { id: "seek".into(), kinds: vec![], weight: 11.0, invert: true },
    ];
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 1, &[0])],
        constraints,
        ..ProblemSpec::new(weeks_grid(&[WeekKind::Exam, WeekKind::Teaching]))
    });

    assert_eq!(cost_at(&problem, 0), 3.0, "inside: the non-inverted instance only");
    assert_eq!(cost_at(&problem, 1), 11.0, "outside: the inverted one only");
}

#[test]
fn exam_weeks_come_from_the_calendar_not_from_slicing_the_week_list() {
    // Re-homed from `soft.rs`, where it lived while the predicate did. The
    // prototype sliced `weeks[-exam_weeks:]`; an exam week in the MIDDLE of the
    // term is what tells the two apart, and the last week being free is what
    // says nothing reintroduced the slice.
    let problem =
        two_level(&[WeekKind::Teaching, WeekKind::Exam, WeekKind::Teaching], vec![], 0, false);

    assert_eq!(cost_at(&problem, 0), 0.0, "week 0 teaches");
    assert_eq!(cost_at(&problem, 1), WEIGHT, "week 1 is the exam week, mid-term");
    assert_eq!(cost_at(&problem, 2), 0.0, "and the LAST week is not one");
}

#[test]
fn scoping_an_exam_week_does_not_reach_the_slot_table() {
    // THE LOAD-BEARING PROPERTY, and the reason the scope lives on
    // `ProblemSpec` rather than on `WeekSpec`. `week_kind` stays a property of
    // a SLOT, never of a `(slot, Group)` pair, so the slot table remains the
    // one coordinate system every constraint resolves against and conflict
    // detection stays a table lookup.
    //
    // A design that pushed the audience axis down into `SlotFlags` would pass
    // every pricing test above and fail here.
    let kinds = [WeekKind::Exam, WeekKind::Teaching, WeekKind::Exam];
    let unscoped = two_level(&kinds, vec![], 0, false);
    let scoped = two_level(&kinds, vec![exam_scope(0, &[0]), exam_scope(2, &[1])], 0, false);

    for slot in unscoped.slots.all() {
        let a = unscoped.slots.flags(slot);
        let b = scoped.slots.flags(slot);
        assert_eq!(a.week, b.week);
        assert_eq!(a.week_kind, b.week_kind, "slot {slot:?} kind must not depend on any scope");
        assert_eq!(a.is_closed(), b.is_closed(), "and an exam week stays OPEN either way");
    }

    // And the scope is not simply being ignored: the mask it feeds differs.
    assert_eq!(cost_at(&unscoped, 2), WEIGHT, "week 2 is an exam week for everybody");
    assert_eq!(cost_at(&scoped, 2), 0.0, "and for the seminar only once scoped");
}

// ---------------------------------------------------------------------------
// Reporting and drift
// ---------------------------------------------------------------------------

#[test]
fn the_breakdown_reports_exam_week_with_the_count_the_objective_charged() {
    // Leaving `SoftParams` drops a type out of `soft_breakdown` unless the
    // function grows a branch for it — the regression the rest of the near-miss
    // family already has. ADR-0024: the breakdown is what the app shows a human
    // to explain the score, so a term that moves the number invisibly is worse
    // than one that does not exist.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 2, &[0])],
        constraints: with_exam_week(testing::all_constraints(), WEIGHT, false),
        // Both weeks are exam weeks, so both placements are charged wherever
        // the search puts them and the expected number is not seed-dependent.
        ..ProblemSpec::new(weeks_grid(&[WeekKind::Exam, WeekKind::Exam]))
    });

    let (solution, soft) = solved(&problem, 5_000);
    let component = soft_breakdown(&problem, &solution)
        .into_iter()
        .find(|c| c.constraint_type == "MinimizeExamWeek")
        .expect("the type must appear in the breakdown");

    assert_eq!(component.constraint_id, "c-exam-week");
    assert_eq!(component.raw_count, 2, "both placements sit in an exam week");
    assert_eq!(component.weighted, 2.0 * WEIGHT);
    assert_eq!(component.weighted, soft, "and that is the whole soft total here");
}

#[test]
fn the_maintained_objective_matches_a_recomputation_over_a_scoped_calendar() {
    // The house guard. A missed read site — the incremental sum in `Trial`, the
    // ranking in `ruin`, the evaluator's delta — shows up here and essentially
    // nowhere else.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        groups: vec![group("A", None), group("B", Some(0))],
        offerings: vec![
            testing::with_groups(testing::offering("for-A", 3, &[0, 1]), &[0]),
            testing::with_groups(testing::offering("for-B", 3, &[0, 1]), &[1]),
        ],
        constraints: with_exam_week(testing::all_constraints(), WEIGHT, false),
        exam_week_groups: vec![exam_scope(0, &[0]), exam_scope(2, &[1])],
        ..ProblemSpec::new(weeks_grid(&[
            WeekKind::Exam,
            WeekKind::Teaching,
            WeekKind::Exam,
            WeekKind::Teaching,
        ]))
    });

    let outcome = solve(&problem, SEED, moves(20_000), &NeverHalt);
    let fresh = recompute_objective(&problem, &outcome.solution);
    assert!(
        objectives_agree(outcome.objective, fresh),
        "maintained {:?} vs recomputed {:?}",
        outcome.objective,
        fresh
    );
}
