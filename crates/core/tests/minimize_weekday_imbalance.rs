//! `MinimizeWeekdayImbalance`: a Group's Sessions should be spread evenly
//! across the active weekdays of a week, not clustered onto a few of them.
//! No parameters beyond the usual id/kinds/weight — the variance is read
//! straight off `TimeGrid.active_days`.

use calendry_solver_core::aggregates::MinimizeWeekdayImbalanceInstance;
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{
    NeverHalt, Trial, objectives_agree, recompute_objective, solve,
};
use calendry_solver_core::slots::SlotTable;
use calendry_solver_core::testing;

mod common;
use common::{SEED, moves};

const TARGET_KIND: &str = "target";

fn target_offering(id: &str, group: u32) -> calendry_solver_core::problem::OfferingSpec {
    calendry_solver_core::problem::OfferingSpec {
        kind: TARGET_KIND.into(),
        ..testing::with_groups(testing::offering(id, 1, &[0, 1]), &[group])
    }
}

fn rule(weight: f64) -> MinimizeWeekdayImbalanceInstance {
    MinimizeWeekdayImbalanceInstance {
        id: "c-imbalance".into(),
        kinds: vec![TARGET_KIND.into()],
        weight,
    }
}

#[test]
fn the_search_spreads_sessions_evenly_across_active_days() {
    // 2 active days (Monday, Tuesday), 1 block each, one Group, two
    // target-kind Offerings — one session per day gives variance 0.
    let grid = SlotTable::build(1, &[1, 2], &testing::teaching_weeks(1)).unwrap();
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        groups: vec![testing::group("G", None)],
        offerings: vec![target_offering("E0", 0), target_offering("E1", 0)],
        constraints: ConstraintSet {
            minimize_weekday_imbalance: vec![rule(10.0)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(grid)
    });

    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.imbalance_cost, 0.0,
        "one session on each of the two active days gives variance 0"
    );
}

#[test]
fn a_non_covered_kind_is_not_counted() {
    // 2 active days, 2 blocks each, one Group, two target-kind Offerings
    // plus one ordinary-kind Offering also attending the Group. Splitting
    // the two target Sessions 1-1 across days costs 0 regardless of where
    // the ordinary-kind Session lands, since it is outside `kinds`.
    let grid = SlotTable::build(2, &[1, 2], &testing::teaching_weeks(1)).unwrap();
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        groups: vec![testing::group("G", None)],
        offerings: vec![
            target_offering("E0", 0),
            target_offering("E1", 0),
            testing::with_groups(testing::offering("L0", 1, &[0, 1]), &[0]),
        ],
        constraints: ConstraintSet {
            minimize_weekday_imbalance: vec![rule(10.0)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(grid)
    });

    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.imbalance_cost, 0.0,
        "the ordinary-kind Session is not `target`-kind and must not count"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_minimize_weekday_imbalance() {
    for seed in 0..8u64 {
        let grid = SlotTable::build(2, &[1, 2], &testing::teaching_weeks(2)).unwrap();
        let offerings = vec![
            target_offering("E0", 0),
            target_offering("E1", 0),
            target_offering("E2", 1),
            target_offering("E3", 1),
        ];
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            groups: vec![testing::group("G0", None), testing::group("G1", None)],
            offerings,
            constraints: ConstraintSet {
                minimize_weekday_imbalance: vec![rule(4.0)],
                ..testing::structural_room_only()
            },
            ..ProblemSpec::new(grid)
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

#[test]
fn a_zero_weight_still_tracks_but_does_not_steer() {
    // 4 target-kind Sessions for one Group, 2 active days x 2 blocks each —
    // enough room for greedy first-fit construction to pack them all onto
    // the first day before touching the second.
    fn problem_with(weight: f64) -> calendry_solver_core::problem::Problem {
        let grid = SlotTable::build(2, &[1, 2], &testing::teaching_weeks(1)).unwrap();
        testing::assemble(ProblemSpec {
            rooms: testing::rooms(2),
            groups: vec![testing::group("G", None)],
            offerings: vec![
                target_offering("E0", 0),
                target_offering("E1", 0),
                target_offering("E2", 0),
                target_offering("E3", 0),
            ],
            constraints: ConstraintSet {
                minimize_weekday_imbalance: vec![rule(weight)],
                ..testing::structural_room_only()
            },
            ..ProblemSpec::new(grid)
        })
    }

    let zero = problem_with(0.0);
    let outcome = solve(&zero, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.imbalance_cost, 0.0, "weight 0 charges nothing");

    // Same instance, weighted, checked right after CONSTRUCTION (no LNS) to
    // confirm the underlying tracking is live: greedy first-fit fills the
    // first day's slots before the second, which is exactly the clustering
    // this type charges for.
    let weighted = problem_with(5.0);
    let unsteered = recompute_objective(&weighted, Trial::construct(&weighted).solution());
    assert!(
        unsteered.imbalance_cost > 0.0 || unsteered.unplaced > 0,
        "greedy first-fit packs Sessions onto the earliest active day before moving to the next"
    );
}
