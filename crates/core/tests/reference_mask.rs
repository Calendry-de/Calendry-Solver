//! ADR-0032: no NEW placement may start before `ProblemSpec::reference`.
//!
//! The mask lives in `SearchState::statically_blocked`, the same
//! occupancy-independent gate as the calendar closure — so construction,
//! repair scoring and the targeted ruin operator all read one definition,
//! and these tests only need to pin the two ends: the earliest legal slot
//! wins, and a fully elapsed grid is reported honestly instead of filled.

use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{Problem, ProblemSpec};
use calendry_solver_core::{Placement, testing};

mod common;
use common::solve_to_convergence as run;

/// One Offering, one Session, one room, four slots — with everything before
/// `reference` elapsed.
fn masked_problem(reference: u32) -> Problem {
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("A", 1, &[0])],
        constraints: testing::structural_room_only(),
        reference: Some(SlotIdx(reference)),
        ..ProblemSpec::new(testing::grid(4, 1))
    })
}

#[test]
fn construction_skips_elapsed_slots() {
    // Greedy wants slot 0; slots 0 and 1 are elapsed time, so the earliest
    // legal slot is exactly the reference itself.
    let outcome = run(&masked_problem(2));

    assert!(outcome.hard_violations.is_empty());
    assert_eq!(
        outcome.solution.get(PlacementIdx(0)),
        Some(Placement::single(SlotIdx(2), RoomIdx(0))),
        "must land on the first slot at or after the reference"
    );
    assert_eq!(outcome.termination_reason, "converged");
}

#[test]
fn a_fully_elapsed_grid_reports_stagnated_not_converged() {
    // Reference one past the last slot: the term is over, nothing is
    // placeable, and the honest answer is unplaced demand — never a placement
    // squeezed into the past, and never `converged` (ADR-0031).
    let outcome = run(&masked_problem(4));

    assert_eq!(outcome.solution.placed_count(), 0);
    assert_eq!(outcome.objective.unplaced, 1);
    assert_eq!(outcome.termination_reason, "stagnated");
}

#[test]
fn no_reference_masks_nothing() {
    // The fixtures' and benchmark generator's default: `None` means no
    // reference exists, not "everything is past" — that wire-level reading
    // belongs to the conversion layer, which maps it to one-past-the-last-
    // slot instead.
    let outcome = run(&testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("A", 1, &[0])],
        constraints: testing::structural_room_only(),
        ..ProblemSpec::new(testing::grid(4, 1))
    }));

    assert_eq!(
        outcome.solution.get(PlacementIdx(0)),
        Some(Placement::single(SlotIdx(0), RoomIdx(0))),
        "greedy takes the first slot when nothing is elapsed"
    );
}
