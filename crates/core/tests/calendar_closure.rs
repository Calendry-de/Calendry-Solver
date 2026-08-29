//! A Break week, a Holiday week, or an individual holiday day inside an
//! otherwise-teaching week: the institution is closed, and no movable Session
//! may land there — unconditionally, not gated by any `Enforce` flag or
//! catalogue constraint.
//!
//! This was a real gap, not a hypothetical one: `WeekKind::Break` /
//! `WeekKind::Holiday` were parsed off the wire and `SlotFlags.is_holiday` was
//! precomputed for every slot, but nothing in the solver ever read either to
//! keep a Session out. `nth_start`/`lower_bound` enumerated every slot
//! uniformly, and the only reader of `week_kind` at all was `MinimizeExamWeek`
//! — which only ever looks at `Exam`, and only as a soft penalty. An ordinary
//! lesson could be placed on a declared holiday and nothing would notice.
//!
//! `Exam` is deliberately NOT closed — an exam period is still open, merely
//! penalized for ordinary lessons (or, with `MinimizeExamWeek.invert`, sought
//! by exam-kind ones). Only `Break`, `Holiday`, and a per-day `is_holiday` flag
//! close a slot.
//!
//! Existing (fixed/locked) occupancy is deliberately untouched: this gates
//! where a NEW placement may go, the same way a locked Session is never
//! second-guessed elsewhere in this codebase.

use calendry_solver_core::ConstraintType;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::ProblemSpec;
use calendry_solver_core::solution::{Placement, SearchState};
use calendry_solver_core::testing;

mod common;
use common::solve_with_move_budget as run;

#[test]
fn a_closed_slot_is_never_chosen_over_an_open_one() {
    // Slot 0 is a Break week, slot 1 is Teaching. Greedy's unweighted "earliest
    // slot" default would pick slot 0 if nothing gated it — so landing on slot
    // 1 instead proves the closure is actively avoided, not merely that the
    // search had no reason to prefer slot 0.
    let problem =
        testing::single_session(testing::break_then_teaching_grid(), testing::rooms(1), vec![]);
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(
        outcome.solution.get(PlacementIdx(0)).unwrap().start.0,
        1,
        "the Break week must never be chosen while an open slot exists"
    );
}

#[test]
fn a_session_confined_to_a_closed_week_is_left_unplaced() {
    // The ENTIRE grid is a Break week: there is no open slot at all. A search
    // that treated closure as a mere preference would still place it here;
    // the correct behaviour is to leave it unplaced and report the shortfall,
    // exactly like any other infeasible instance.
    let problem = testing::single_session(testing::break_week_grid(), testing::rooms(1), vec![]);
    let outcome = run(&problem);

    assert!(
        outcome.solution.get(PlacementIdx(0)).is_none(),
        "a Break week must never be used even as a last resort"
    );
    assert!(
        outcome
            .hard_violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::ExactFrequency),
        "the shortfall must be reported, not silently swallowed: {:?}",
        outcome.hard_violations
    );
}

#[test]
fn the_same_grid_without_the_break_kind_places_it_fine() {
    // Isolates the closure as the cause of the test above, rather than some
    // other property of a one-slot, one-room instance.
    let problem = testing::single_session(testing::grid(1, 1), testing::rooms(1), vec![]);
    let outcome = run(&problem);

    assert!(outcome.solution.get(PlacementIdx(0)).is_some());
    assert!(outcome.hard_violations.is_empty());
}

#[test]
fn a_holiday_day_inside_a_teaching_week_closes_only_that_day() {
    // Two weekdays, both Teaching, but Tuesday is individually a holiday.
    // Monday must still be usable — closure is per-slot, not per-week, when
    // the WeekKind itself is Teaching.
    use calendry_solver_core::slots::{SlotTable, WeekKind, WeekSpec};

    let grid = SlotTable::build(
        1,
        &[1, 2],
        &[WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![2] }],
    )
    .unwrap();
    let problem = testing::single_session(grid, testing::rooms(1), vec![]);
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(
        outcome.solution.get(PlacementIdx(0)).unwrap().start.0,
        0,
        "Monday (slot 0) is open; Tuesday (slot 1) is an individual holiday"
    );
}

#[test]
fn fixed_occupancy_already_on_a_closed_slot_is_left_alone() {
    // A locked Session's own data is never second-guessed by this codebase —
    // ADR-0008's hard-lock policy applies regardless of what the slot's
    // calendar flags say. Fixed occupancy is marked directly, never through
    // `is_free`, so building the state must succeed without erroring or
    // silently relocating it — and a genuinely movable Session in the SAME
    // run must still solve normally around it, proving the closed/occupied
    // slot did not corrupt anything for the rest of the instance.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        fixed: vec![testing::fixed_session("locked-on-break", Some(0), 0)],
        offerings: vec![testing::offering("S", 1, &[0])],
        ..ProblemSpec::new(testing::break_then_teaching_grid())
    });

    // Building state from fixed occupancy must not panic on a closed slot.
    let state = SearchState::from_fixed(&problem);
    assert!(
        !state.can_place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))),
        "slot 0 is both closed and already occupied by the fixed Session"
    );

    let outcome = run(&problem);
    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(
        outcome.solution.get(PlacementIdx(0)).unwrap().start.0,
        1,
        "the movable Session must land on the open slot, undisturbed by the locked one"
    );
}

#[test]
fn evaluate_hard_still_reports_a_genuine_shortfall_and_nothing_about_closure_itself() {
    // Closure is not itself a `ConstraintType` — it is unconditional grid
    // behaviour, like refusing a Session that would spill past the end of a
    // day. So there is no "closure violation" to look for; this pins that the
    // ONLY violation a confined instance produces is the ordinary
    // `ExactFrequency` shortfall, not some new, undocumented variant.
    let problem = testing::single_session(testing::break_week_grid(), testing::rooms(1), vec![]);
    let outcome = run(&problem);

    assert_eq!(outcome.hard_violations.len(), 1);
    assert_eq!(outcome.hard_violations[0].constraint_type, ConstraintType::ExactFrequency);
}
