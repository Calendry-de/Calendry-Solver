//! `MinimizeOfferingDistinctDays`: the "multiple in a day" per-Offering
//! teaching mode (issue #28) — prices the number of DISTINCT days one
//! Offering's Sessions land on across the whole term, for Offerings tagged
//! `Offering.prefer_fuller_days`. Independent of `scheduling_pattern` (WHEN
//! in the term): the two combine, so BLOCK + this means a short, dense run
//! of full days.
//!
//! Same "distinct nonzero cells minus one" reduction
//! `DistributedPatternAdherence` uses, at DAY granularity across the term
//! instead of weekly-cell.

use calendry_solver_core::aggregates::PatternAdherenceInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn rule(weight: f64) -> ConstraintSet {
    ConstraintSet {
        minimize_offering_distinct_days: vec![PatternAdherenceInstance {
            id: "c-days".into(),
            kinds: vec![],
            weight,
        }],
        ..testing::structural_room_only()
    }
}

#[test]
fn three_distinct_days_costs_weight_times_two() {
    // 3 blocks/day, 3 weeks, Monday only: day_index 0, 1, 2 are three
    // distinct calendar days, one per week.
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![testing::with_fuller_days(testing::offering("O", 3, &[0]))],
        constraints: rule(5.0),
        ..ProblemSpec::new(testing::grid(3, 3))
    });

    let mut solution = Solution::empty(&problem);
    // One Session per week, all at block 0: three distinct days.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0)))); // week 0
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(3), RoomIdx(0)))); // week 1
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(6), RoomIdx(0)))); // week 2

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_distinct_days_cost, 10.0, "3 distinct days: weight * (3-1)");
}

#[test]
fn all_three_sessions_on_one_day_costs_nothing() {
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![testing::with_fuller_days(testing::offering("O", 3, &[0]))],
        constraints: rule(5.0),
        ..ProblemSpec::new(testing::grid(3, 3))
    });

    let mut solution = Solution::empty(&problem);
    // All three Sessions land in week 0's three blocks: one distinct day.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_distinct_days_cost, 0.0, "1 distinct day: weight * (1-1)");
}

#[test]
fn an_offering_not_tagged_prefer_fuller_days_is_never_charged() {
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![testing::offering("O", 3, &[0])], // NOT tagged
        constraints: rule(5.0),
        ..ProblemSpec::new(testing::grid(3, 3))
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(3), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(6), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_distinct_days_cost, 0.0);
}

#[test]
fn no_instance_configured_costs_nothing_even_when_tagged() {
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![testing::with_fuller_days(testing::offering("O", 3, &[0]))],
        constraints: testing::structural_room_only(), // no rule
        ..ProblemSpec::new(testing::grid(3, 3))
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(3), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(6), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.offering_distinct_days_cost, 0.0);
}

#[test]
fn the_search_packs_a_tagged_offering_onto_fewer_days() {
    // Nothing else distinguishes candidate slots, so a tagged Offering's 3
    // required Sessions should be packed onto as few distinct days as the
    // grid allows (one day, since blocks_per_day = 3) rather than spread
    // one-per-week.
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![testing::with_fuller_days(testing::offering("O", 3, &[0]))],
        constraints: rule(5.0),
        ..ProblemSpec::new(testing::grid(3, 3))
    });

    let outcome = solve(&problem, SEED, moves(2_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.offering_distinct_days_cost, 0.0,
        "packing all 3 Sessions onto one day is reachable and must win"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_offering_distinct_days() {
    for seed in 0..8u64 {
        let spec = ProblemSpec {
            rooms: vec![testing::room("R0"), testing::room("R1")],
            offerings: vec![
                testing::with_fuller_days(testing::offering("A", 4, &[0, 1])),
                testing::with_fuller_days(testing::offering("B", 3, &[0, 1])),
            ],
            constraints: rule(5.0),
            ..ProblemSpec::new(testing::grid(4, 3))
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
