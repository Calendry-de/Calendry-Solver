//! `RoomConsistency`: an Offering's recurring Sessions should reuse the same
//! Room each week rather than bouncing around — mirrors `LecturerConsistency`
//! but for the Room, and has no prerequisite blocker since Room assignment is
//! not gated behind an unimplemented pool-selection feature.
//!
//! An aggregate over an entire Offering's Sessions across the WHOLE TERM,
//! keyed by Offering rather than Group and unbounded by day or window — the
//! same new shape `DistributedPatternAdherence`/`BlockPatternAdherence`
//! already use. The "usual" Room is the MODAL one among an Offering's
//! currently-placed Sessions; every Session differing from it is priced.

use calendry_solver_core::aggregates::RoomConsistencyInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn consistency_rule(weight: f64) -> RoomConsistencyInstance {
    RoomConsistencyInstance { id: "c-consistency".into(), kinds: vec![], weight }
}

/// One Offering, 3 required Sessions, 2 Rooms, 3 slots — plenty of room to
/// place all three Sessions with no structural pressure to pick any
/// particular Room.
fn one_offering_three_sessions(weight: f64) -> calendry_solver_core::Problem {
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        offerings: vec![testing::offering("O", 3, &[0, 1])],
        constraints: ConstraintSet {
            room_consistency: vec![consistency_rule(weight)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(3, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = one_offering_three_sessions(5.0);
    let mut solution = Solution::empty(&problem);
    // 2 Sessions in Room0 (the modal Room), 1 in Room1: excess = 3 - 2 = 1,
    // cost = weight * excess = 5 * 1 = 5.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_consistency_cost, 5.0);
}

#[test]
fn every_session_in_the_same_room_costs_nothing() {
    let problem = one_offering_three_sessions(5.0);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_consistency_cost, 0.0);
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_room_variety() {
    let problem = one_offering_three_sessions(0.0);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_consistency_cost, 0.0);
}

#[test]
fn the_search_keeps_an_offerings_sessions_in_one_room() {
    let problem = one_offering_three_sessions(10.0);
    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.room_consistency_cost, 0.0,
        "2 interchangeable Rooms and no other pressure always allow one Room for all 3 Sessions"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_room_consistency() {
    for seed in 0..8u64 {
        let spec = ProblemSpec {
            rooms: testing::rooms(3),
            offerings: vec![
                testing::offering("A", 3, &[0, 1, 2]),
                testing::offering("B", 2, &[0, 1, 2]),
            ],
            constraints: ConstraintSet {
                room_consistency: vec![consistency_rule(4.0)],
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
