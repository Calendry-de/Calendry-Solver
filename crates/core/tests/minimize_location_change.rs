//! `MinimizeLocationChange`: penalizes a Group's or Person's day for touching
//! more than a configured number of distinct `Room.location` values —
//! reduces cross-campus walking between back-to-back Sessions.

use calendry_solver_core::aggregates::MinimizeLocationChangeInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn location_rule(weight: f64, max_locations_per_day: u32) -> MinimizeLocationChangeInstance {
    MinimizeLocationChangeInstance {
        id: "c-location".into(),
        kinds: vec![],
        weight,
        group: true,
        person: false,
        max_locations_per_day,
    }
}

/// Two Rooms in location "A" (`R0`, `R1`) and one in location "B" (`R2`), two
/// single-session Offerings sharing one Group.
fn two_sessions_one_group(
    weight: f64,
    max_locations_per_day: u32,
) -> calendry_solver_core::Problem {
    let offerings = (0..2)
        .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 1, &[0, 1, 2]), &[0]))
        .collect();

    testing::assemble(ProblemSpec {
        rooms: vec![
            testing::room_at("R0", "A"),
            testing::room_at("R1", "A"),
            testing::room_at("R2", "B"),
        ],
        groups: vec![testing::group("G", None)],
        offerings,
        constraints: ConstraintSet {
            minimize_location_change: vec![location_rule(weight, max_locations_per_day)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(6, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = two_sessions_one_group(5.0, 1);
    let mut solution = Solution::empty(&problem);
    // R0 is location "A", R2 is location "B": 2 distinct locations that day,
    // excess over cap 1 is 1, cost = weight * excess = 5 * 1 = 5.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(2))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.location_change_cost, 5.0);
}

#[test]
fn staying_within_one_location_costs_nothing() {
    let problem = two_sessions_one_group(5.0, 1);
    let mut solution = Solution::empty(&problem);
    // R0 and R1 are BOTH location "A": one distinct location, no excess.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.location_change_cost, 0.0);
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_locations() {
    let problem = two_sessions_one_group(0.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(2))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.location_change_cost, 0.0);
}

#[test]
fn a_sessions_own_rooms_in_one_location_count_once() {
    // A single Session split across R0 and R1 — both location "A" — must
    // touch exactly ONE distinct location, not two, even though it occupies
    // two Rooms simultaneously.
    let problem = two_sessions_one_group(5.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(
        PlacementIdx(0),
        Some(Placement::with_rooms(SlotIdx(0), RoomIdx(0), [Some(RoomIdx(1)), None, None])),
    );

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(
        objective.location_change_cost, 0.0,
        "R0 and R1 share a location, so this Session touches only one"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_minimize_location_change() {
    for seed in 0..8u64 {
        let rooms = vec![
            testing::room_at("R0", "A"),
            testing::room_at("R1", "B"),
            testing::room_at("R2", "C"),
        ];
        let offerings = (0..5)
            .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 2, &[0, 1, 2]), &[0]))
            .collect();
        let spec = ProblemSpec {
            rooms,
            groups: vec![testing::group("G", None)],
            offerings,
            constraints: ConstraintSet {
                minimize_location_change: vec![location_rule(3.0, 1)],
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
