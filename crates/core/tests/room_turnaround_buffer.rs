//! `RoomTurnaroundBuffer`: requires a minimum gap between two bookings of the
//! SAME Room, even when the slot grid would otherwise allow them back-to-back
//! — labs needing equipment reset, or any Room needing setup/teardown time.
//!
//! Genuinely new shape: pairwise like the four structural double-booking
//! types, but keyed by a configurable BUFFER DISTANCE rather than exact-slot
//! overlap, and Room-keyed rather than Group/Person-keyed.

use calendry_solver_core::aggregates::RoomTurnaroundBufferInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn buffer_rule(weight: f64, buffer_blocks: u32) -> RoomTurnaroundBufferInstance {
    RoomTurnaroundBufferInstance { id: "c-buffer".into(), kinds: vec![], weight, buffer_blocks }
}

/// One Room, two single-block single-session Offerings — nothing else
/// attends either, so only the Room axis is in play.
fn two_sessions_one_room(weight: f64, buffer_blocks: u32) -> calendry_solver_core::Problem {
    let offerings = (0..2)
        .map(|i| testing::offering(&format!("O{i}"), 1, &[0]))
        .collect::<Vec<_>>();

    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings,
        constraints: ConstraintSet {
            room_turnaround_buffer: vec![buffer_rule(weight, buffer_blocks)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(6, 1))
    })
}

#[test]
fn back_to_back_bookings_violate_any_configured_buffer() {
    // Blocks 0 and 1 (immediately adjacent, zero gap) with buffer_blocks=1 —
    // one violated boundary. A plain occupancy count cannot see the A/B
    // boundary at all once merged, which is exactly the case this type must
    // still catch.
    let problem = two_sessions_one_room(5.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_turnaround_cost, 5.0);
}

#[test]
fn a_gap_meeting_the_buffer_costs_nothing() {
    // Blocks 0 and 2: exactly one free block between them, buffer_blocks=1.
    let problem = two_sessions_one_room(5.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_turnaround_cost, 0.0);
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_the_gap() {
    let problem = two_sessions_one_room(0.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_turnaround_cost, 0.0);
}

#[test]
fn an_unconfigured_buffer_never_fires() {
    // buffer_blocks=0 means "no separation required" — the inert default.
    let problem = two_sessions_one_room(5.0, 0);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_turnaround_cost, 0.0);
}

#[test]
fn the_search_spreads_two_sessions_apart_to_satisfy_the_buffer() {
    // Plenty of headroom (6 blocks, one Room) for the search to find a gap
    // meeting buffer_blocks=2.
    let problem = two_sessions_one_room(10.0, 2);
    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.room_turnaround_cost, 0.0,
        "6 blocks in one Room always has a pair at least 2 apart"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_room_turnaround_buffer() {
    for seed in 0..8u64 {
        let offerings = (0..5)
            .map(|i| testing::offering(&format!("O{i}"), 2, &[0, 1]))
            .collect::<Vec<_>>();
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            offerings,
            constraints: ConstraintSet {
                room_turnaround_buffer: vec![buffer_rule(3.0, 1)],
                ..testing::structural_room_only()
            },
            ..ProblemSpec::new(testing::grid(3, 4))
        };
        let problem = testing::assemble(spec);
        let outcome = solve(&problem, SEED ^ seed, moves(500), &NeverHalt);
        let full = recompute_objective(&problem, &outcome.solution);
        assert!(
            objectives_agree(outcome.objective, full),
            "seed {seed}: drifted, incremental {:?} vs recomputed {:?}",
            outcome.objective,
            full
        );
    }
}
