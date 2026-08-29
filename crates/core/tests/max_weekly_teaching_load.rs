//! `MaxWeeklyTeachingLoad`: caps how many Sessions (or blocks) a lecturer
//! teaches in one week. Lecturer-only, no Group/Person axis split — unlike
//! `Compactness` and its siblings, there is no ambiguity about whose load is
//! being capped.

use calendry_solver_core::aggregates::MaxWeeklyTeachingLoadInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn load_rule(weight: f64, count_blocks: bool, max_per_week: u32) -> MaxWeeklyTeachingLoadInstance {
    MaxWeeklyTeachingLoadInstance {
        id: "c-load".into(),
        kinds: vec![],
        weight,
        count_blocks,
        max_per_week,
    }
}

fn two_sessions_one_lecturer(weight: f64, max_per_week: u32) -> calendry_solver_core::Problem {
    let offerings = (0..2)
        .map(|i| testing::with_lecturers(testing::offering(&format!("O{i}"), 1, &[0]), &[0]))
        .collect();

    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: ConstraintSet {
            max_weekly_teaching_load: vec![load_rule(weight, false, max_per_week)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(2, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = two_sessions_one_lecturer(5.0, 1);
    let mut solution = Solution::empty(&problem);
    // Both Sessions land in the only week; the lecturer teaches 2, cap is 1,
    // excess 1, cost = weight * excess = 5 * 1 = 5.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_weekly_teaching_load_cost, 5.0);
}

#[test]
fn within_the_cap_costs_nothing() {
    let problem = two_sessions_one_lecturer(5.0, 2);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_weekly_teaching_load_cost, 0.0);
}

#[test]
fn count_blocks_counts_duration_not_sessions() {
    // One Offering, one Session, 2 blocks long; cap of 1 SESSION would read
    // this as "1", fine — but with count_blocks the same Session counts as
    // 2, over a cap of 1.
    let offering = testing::with_lecturers(testing::offering("O", 1, &[0]), &[0]);
    let mut offering = offering;
    offering.duration_blocks = 2;
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person("P", &[])],
        offerings: vec![offering],
        constraints: ConstraintSet {
            max_weekly_teaching_load: vec![load_rule(5.0, true, 1)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(2, 1))
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_weekly_teaching_load_cost, 5.0, "2 blocks over a cap of 1");
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_load() {
    let problem = two_sessions_one_lecturer(0.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_weekly_teaching_load_cost, 0.0);
}

#[test]
fn incremental_objective_matches_full_recomputation_with_max_weekly_teaching_load() {
    for seed in 0..8u64 {
        let offerings = (0..6)
            .map(|i| {
                testing::with_lecturers(
                    testing::offering(&format!("O{i}"), 2, &[0, 1]),
                    &[(i % 2) as u32],
                )
            })
            .collect();
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            persons: vec![testing::person("P0", &[]), testing::person("P1", &[])],
            offerings,
            constraints: ConstraintSet {
                max_weekly_teaching_load: vec![load_rule(3.0, false, 2)],
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
