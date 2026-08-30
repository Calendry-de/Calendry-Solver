//! `MaxDailySessionCount`: caps a raw Session COUNT per day for a Group
//! and/or a Person — the volume-limit sibling of `MaxDailySpan` (elapsed
//! time) and `MaxConsecutiveBlocks` (continuity): a day can satisfy both of
//! those and still be overloaded, e.g. 6 lessons split 3 + gap + 3.

use calendry_solver_core::aggregates::MaxDailySessionCountInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn count_rule(
    group: bool,
    person: bool,
    weight: f64,
    max_per_day: u32,
) -> MaxDailySessionCountInstance {
    MaxDailySessionCountInstance {
        id: "c-daily-count".into(),
        kinds: vec![],
        weight,
        group,
        person,
        max_per_day,
    }
}

/// 3 Offerings, one Session each, one Group, all eligible on the same day —
/// plenty of room to spread across days OR pile onto one.
fn three_offerings_one_group(weight: f64, max_per_day: u32) -> calendry_solver_core::Problem {
    let offerings = (0..3)
        .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 1, &[0]), &[0]))
        .collect();

    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![testing::group("G", None)],
        offerings,
        constraints: ConstraintSet {
            max_daily_session_count: vec![count_rule(true, false, weight, max_per_day)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(6, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = three_offerings_one_group(5.0, 1);
    let mut solution = Solution::empty(&problem);
    // All 3 Sessions on the same (only) day: count 3, excess over cap 1 is
    // 2, cost = weight * excess = 5 * 2 = 10.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_daily_session_count_cost, 10.0);
}

#[test]
fn a_count_within_the_cap_costs_nothing() {
    let problem = three_offerings_one_group(5.0, 3);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_daily_session_count_cost, 0.0, "3 at the cap of 3 has no excess");
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_count() {
    let problem = three_offerings_one_group(0.0, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.max_daily_session_count_cost, 0.0);
}

#[test]
fn the_search_spreads_offerings_across_days_to_satisfy_the_cap() {
    // 6 blocks/day but only 1 active day in this fixture's grid would force a
    // violation regardless, so use a grid with several active days instead —
    // spreading is only a real choice when days actually exist to spread
    // into.
    let offerings = (0..3)
        .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 1, &[0]), &[0]))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![testing::group("G", None)],
        offerings,
        constraints: ConstraintSet {
            max_daily_session_count: vec![count_rule(true, false, 10.0, 1)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(2, 3))
    });

    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.max_daily_session_count_cost, 0.0,
        "3 weeks of days available and no other pressure always allows one Session per day"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_max_daily_session_count() {
    for seed in 0..8u64 {
        let offerings = (0..5)
            .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 2, &[0, 1]), &[0]))
            .collect();
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            groups: vec![testing::group("G", None)],
            offerings,
            constraints: ConstraintSet {
                max_daily_session_count: vec![count_rule(true, true, 3.0, 1)],
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
