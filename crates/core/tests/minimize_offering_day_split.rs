//! `MinimizeOfferingDaySplit`: prices the number of non-contiguous runs of
//! one Offering's Sessions within a day, minus one. NOT the same question
//! `Compactness` asks: a day packed solid with unrelated teaching in
//! between two runs of the same Offering has zero gaps and still splits it.
//! Part of the `(Offering, day)` cluster with `MaxOfferingSessionsPerDay`
//! and `MaxConsecutiveOfferingBlocks`.

use calendry_solver_core::aggregates::MinimizeOfferingDaySplitInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn split_rule(weight: f64) -> MinimizeOfferingDaySplitInstance {
    MinimizeOfferingDaySplitInstance { id: "c-off-split".into(), kinds: vec![], weight }
}

/// One Offering, 3 required (single-block) Sessions, 6 blocks in the one
/// active day.
fn one_offering_three_sessions(weight: f64) -> calendry_solver_core::Problem {
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 3, &[0])],
        constraints: ConstraintSet {
            minimize_offering_day_split: vec![split_rule(weight)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(6, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = one_offering_three_sessions(5.0);
    let mut solution = Solution::empty(&problem);
    // Blocks 0, 2, 4: three separate runs, excess = runs - 1 = 2,
    // cost = weight * excess = 5 * 2 = 10.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(2), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(4), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_split_cost, 10.0);
}

#[test]
fn one_contiguous_run_costs_nothing() {
    let problem = one_offering_three_sessions(5.0);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_split_cost, 0.0, "a single run, however long, is not a split");
}

#[test]
fn a_lone_session_costs_nothing() {
    let problem = one_offering_three_sessions(5.0);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_split_cost, 0.0);
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_splits() {
    let problem = one_offering_three_sessions(0.0);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(2), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(4), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_split_cost, 0.0);
}

#[test]
fn the_search_keeps_an_offerings_day_contiguous() {
    let problem = one_offering_three_sessions(10.0);
    let outcome = solve(&problem, SEED, moves(10_000), &NeverHalt);

    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.offering_split_cost, 0.0,
        "3 Sessions in 6 blocks always has a fully contiguous arrangement"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_minimize_offering_day_split() {
    for seed in 0..8u64 {
        let offerings = (0..5)
            .map(|i| testing::offering(&format!("O{i}"), 3, &[0, 1]))
            .collect();
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            offerings,
            constraints: ConstraintSet {
                minimize_offering_day_split: vec![split_rule(3.0)],
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
