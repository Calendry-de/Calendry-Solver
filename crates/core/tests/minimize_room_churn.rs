//! `MinimizeRoomChurn`: caps how many DISTINCT Rooms a Group uses across a
//! whole WEEK — the "home room" concept. Distinct from `MinimizeLocationChange`,
//! which is about distinct LOCATIONS within one day: a Group can churn
//! several Rooms in the same building without ever crossing one.

use calendry_solver_core::aggregates::MinimizeRoomChurnInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn churn_rule(weight: f64, max_rooms_per_week: u32) -> MinimizeRoomChurnInstance {
    MinimizeRoomChurnInstance { id: "c-churn".into(), kinds: vec![], weight, max_rooms_per_week }
}

/// 3 Rooms, one Group, two single-session Offerings both attending it, one
/// week (2 blocks — enough for both Sessions to land on distinct days
/// without any structural pressure to share a Room).
fn two_sessions_one_group(weight: f64, max_rooms_per_week: u32) -> calendry_solver_core::Problem {
    let offerings = (0..2)
        .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 1, &[0, 1, 2]), &[0]))
        .collect();

    testing::assemble(ProblemSpec {
        rooms: testing::rooms(3),
        groups: vec![testing::group("G", None)],
        offerings,
        constraints: ConstraintSet {
            minimize_room_churn: vec![churn_rule(weight, max_rooms_per_week)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(2, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = two_sessions_one_group(5.0, 1);
    let mut solution = Solution::empty(&problem);
    // Two distinct Rooms in the same week, cap 1: excess = 2 - 1 = 1,
    // cost = weight * excess = 5 * 1 = 5.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_churn_cost, 5.0);
}

#[test]
fn staying_in_one_room_all_week_costs_nothing() {
    let problem = two_sessions_one_group(5.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_churn_cost, 0.0);
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_room_variety() {
    let problem = two_sessions_one_group(0.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_churn_cost, 0.0);
}

#[test]
fn a_sessions_own_rooms_in_one_placement_count_once() {
    // A single multi-Room Session split across R0 and R1 must touch exactly
    // TWO distinct Rooms (unlike MinimizeLocationChange's location-string
    // dedup, every distinct Room genuinely counts here) — verified against a
    // cap of 2, which this alone must NOT exceed.
    let problem = two_sessions_one_group(5.0, 2);
    let mut solution = Solution::empty(&problem);
    solution.set(
        PlacementIdx(0),
        Some(Placement::with_rooms(SlotIdx(0), RoomIdx(0), [Some(RoomIdx(1)), None, None])),
    );

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.room_churn_cost, 0.0, "2 distinct Rooms, cap 2, no excess");
}

#[test]
fn the_search_keeps_a_group_in_one_room_when_the_cap_allows_it() {
    let problem = two_sessions_one_group(10.0, 1);
    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.room_churn_cost, 0.0,
        "3 interchangeable Rooms always have a same-Room arrangement"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_minimize_room_churn() {
    for seed in 0..8u64 {
        let offerings = vec![
            testing::with_groups(testing::offering("E0", 2, &[0, 1, 2]), &[0]),
            testing::with_groups(testing::offering("E1", 2, &[0, 1, 2]), &[0]),
            testing::with_groups(testing::offering("E2", 2, &[0, 1, 2]), &[1]),
            testing::with_groups(testing::offering("E3", 2, &[0, 1, 2]), &[1]),
        ];
        let spec = ProblemSpec {
            rooms: testing::rooms(3),
            groups: vec![testing::group("G0", None), testing::group("G1", None)],
            offerings,
            constraints: ConstraintSet {
                minimize_room_churn: vec![churn_rule(4.0, 1)],
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
