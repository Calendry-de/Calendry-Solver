//! `MaxConsecutiveOfferingBlocks`: caps how many blocks of ONE Offering may
//! run back to back in a day, distinguishing an intentional multi-block
//! Session (`Offering.duration_blocks`, one placement) from several separate
//! Sessions of the same Offering landing consecutively by accident. Part of
//! the `(Offering, day)` cluster with `MaxOfferingSessionsPerDay` and
//! `MinimizeOfferingDaySplit`.

use calendry_solver_core::aggregates::MaxConsecutiveOfferingBlocksInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn run_rule(weight: f64, max_consecutive: u32) -> MaxConsecutiveOfferingBlocksInstance {
    MaxConsecutiveOfferingBlocksInstance {
        id: "c-off-run".into(),
        kinds: vec![],
        weight,
        max_consecutive,
    }
}

/// One Offering, 3 required (single-block) Sessions, 6 blocks in the one
/// active day — enough headroom to break the run apart (e.g. 0, 1, 3).
fn one_offering_three_sessions(weight: f64, max_consecutive: u32) -> calendry_solver_core::Problem {
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 3, &[0])],
        constraints: ConstraintSet {
            max_consecutive_offering_blocks: vec![run_rule(weight, max_consecutive)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(6, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = one_offering_three_sessions(5.0, 1);
    let mut solution = Solution::empty(&problem);
    // Blocks 0, 1, 2: one contiguous run of 3, excess over cap 1 is 2,
    // cost = weight * excess = 5 * 2 = 10.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_run_cost, 10.0);
}

#[test]
fn a_run_within_the_cap_costs_nothing() {
    let problem = one_offering_three_sessions(5.0, 3);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_run_cost, 0.0, "3 blocks at the cap of 3 has no excess");
}

#[test]
fn splitting_the_run_avoids_the_charge_even_on_the_same_day() {
    let problem = one_offering_three_sessions(5.0, 1);
    let mut solution = Solution::empty(&problem);
    // Blocks 0, 2, 4: three separate single-block runs, each at the cap.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(2), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(4), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_run_cost, 0.0);
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_run_length() {
    let problem = one_offering_three_sessions(0.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_run_cost, 0.0);
}

#[test]
fn the_search_breaks_up_a_long_run() {
    let problem = one_offering_three_sessions(10.0, 1);
    let outcome = solve(&problem, SEED, moves(10_000), &NeverHalt);

    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.offering_run_cost, 0.0,
        "3 Sessions in 6 blocks always has an arrangement with no run over 1; the search must find one"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_max_consecutive_offering_blocks() {
    for seed in 0..8u64 {
        let offerings = (0..5)
            .map(|i| testing::offering(&format!("O{i}"), 3, &[0, 1]))
            .collect();
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            offerings,
            constraints: ConstraintSet {
                max_consecutive_offering_blocks: vec![run_rule(3.0, 1)],
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
