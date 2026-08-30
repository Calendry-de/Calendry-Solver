//! `MaxOfferingSessionsPerDay`: caps a raw Session COUNT of one Offering on
//! one day — "Maths, 4x a week" means four different days unless a tenant
//! says otherwise. The Offering-keyed sibling of `MaxDailySessionCount`
//! (Group/Person axis); part of the `(Offering, day)` cluster with
//! `MaxConsecutiveOfferingBlocks` and `MinimizeOfferingDaySplit`.

use calendry_solver_core::aggregates::MaxOfferingSessionsPerDayInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn count_rule(weight: f64, max_per_day: u32) -> MaxOfferingSessionsPerDayInstance {
    MaxOfferingSessionsPerDayInstance {
        id: "c-off-count".into(),
        kinds: vec![],
        weight,
        max_per_day,
    }
}

/// One Offering, 3 required Sessions, 6 blocks in the one active day —
/// enough headroom to spread across weeks OR pile onto one day.
fn one_offering_three_sessions(weight: f64, max_per_day: u32) -> calendry_solver_core::Problem {
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 3, &[0])],
        constraints: ConstraintSet {
            max_offering_sessions_per_day: vec![count_rule(weight, max_per_day)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(6, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = one_offering_three_sessions(5.0, 1);
    let mut solution = Solution::empty(&problem);
    // All 3 Sessions on the same (only) day: count 3, excess over cap 1 is
    // 2, cost = weight * excess = 5 * 2 = 10.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_daily_count_cost, 10.0);
}

#[test]
fn a_count_within_the_cap_costs_nothing() {
    let problem = one_offering_three_sessions(5.0, 3);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_daily_count_cost, 0.0);
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_count() {
    let problem = one_offering_three_sessions(0.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_daily_count_cost, 0.0);
}

#[test]
fn the_search_spreads_an_offering_across_days_to_satisfy_the_cap() {
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 3, &[0])],
        constraints: ConstraintSet {
            max_offering_sessions_per_day: vec![count_rule(10.0, 1)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(2, 3))
    });

    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.offering_daily_count_cost, 0.0,
        "3 weeks of days available and no other pressure always allows one Session per day"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_max_offering_sessions_per_day() {
    for seed in 0..8u64 {
        let offerings = (0..5)
            .map(|i| testing::offering(&format!("O{i}"), 2, &[0, 1]))
            .collect();
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            offerings,
            constraints: ConstraintSet {
                max_offering_sessions_per_day: vec![count_rule(3.0, 1)],
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
