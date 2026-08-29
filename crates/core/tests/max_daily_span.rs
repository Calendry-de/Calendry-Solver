//! `MaxDailySpan`: caps the elapsed time from a Group's or Person's first to
//! last Session of a day — distinct from both `Compactness` (gaps inside the
//! span) and `MaxConsecutiveBlocks` (density): a day can have zero gaps and
//! low density and still run too long if the bracketing Sessions are simply
//! far apart.

use calendry_solver_core::aggregates::MaxDailySpanInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn span_rule(group: bool, person: bool, weight: f64, max_span_blocks: u32) -> MaxDailySpanInstance {
    MaxDailySpanInstance {
        id: "c-span".into(),
        kinds: vec![],
        weight,
        group,
        person,
        max_span_blocks,
    }
}

fn two_sessions_one_group(weight: f64, max_span_blocks: u32) -> calendry_solver_core::Problem {
    let offerings = (0..2)
        .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 1, &[0]), &[0]))
        .collect();

    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![testing::group("G", None)],
        offerings,
        constraints: ConstraintSet {
            max_daily_span: vec![span_rule(true, false, weight, max_span_blocks)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(6, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = two_sessions_one_group(5.0, 2);
    let mut solution = Solution::empty(&problem);
    // Block 0 and block 4: span = 4 - 0 + 1 = 5, excess over cap 2 is 3,
    // cost = weight * excess = 5 * 3 = 15.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(4), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_daily_span_cost, 15.0);
}

#[test]
fn a_span_within_the_cap_costs_nothing() {
    let problem = two_sessions_one_group(5.0, 2);
    let mut solution = Solution::empty(&problem);
    // Block 0 and block 1: span = 2, exactly at the cap, excess 0.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_daily_span_cost, 0.0);
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_span() {
    let problem = two_sessions_one_group(0.0, 2);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(5), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_daily_span_cost, 0.0);
}

#[test]
fn incremental_objective_matches_full_recomputation_with_max_daily_span() {
    for seed in 0..8u64 {
        let offerings = (0..5)
            .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 2, &[0, 1]), &[0]))
            .collect();
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            groups: vec![testing::group("G", None)],
            offerings,
            constraints: ConstraintSet {
                max_daily_span: vec![span_rule(true, true, 3.0, 2)],
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
