//! `MaxDays` / `MaxConsecutiveDays`: HARD caps on how many distinct days (or
//! how long a consecutive run of days) a Group's or Person's Sessions may
//! use, per week — priced at `hard_penalty` rather than a construction
//! filter (ADR-0025), the same stance `MaxOnlineShare` takes, since
//! `minimize_day_usage` can only discourage, never refuse.
//!
//! Both share the same underlying day-occupancy substrate
//! (`Aggregates::day_cap_group`/`day_cap_person`) — one reduces it by
//! DISTINCT day count, the other by longest CONSECUTIVE run.

use calendry_solver_core::aggregates::{MaxConsecutiveDaysInstance, MaxDaysInstance};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;

mod common;
use common::{SEED, moves};

fn offering(id: &str, count: u32) -> calendry_solver_core::problem::OfferingSpec {
    testing::with_groups(testing::offering(id, count, &[0]), &[0])
}

fn capped_days(max_days: u32) -> ConstraintSet {
    ConstraintSet {
        max_days: vec![MaxDaysInstance {
            id: "c-days".into(),
            kinds: vec![],
            group: true,
            person: false,
            max_days,
        }],
        ..testing::structural_room_only()
    }
}

fn capped_consecutive_days(max_consecutive_days: u32) -> ConstraintSet {
    ConstraintSet {
        max_consecutive_days: vec![MaxConsecutiveDaysInstance {
            id: "c-consec-days".into(),
            kinds: vec![],
            group: true,
            person: false,
            max_consecutive_days,
        }],
        ..testing::structural_room_only()
    }
}

#[test]
fn a_week_within_the_cap_is_not_reported() {
    // 1 block/day grid, 5 active days: 2 required Sessions use at most 2
    // distinct days, within a cap of 3.
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        groups: vec![testing::group("G", None)],
        offerings: vec![offering("O", 2)],
        constraints: capped_days(3),
        ..ProblemSpec::new(testing::grid_5day(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(outcome.objective.max_days_violations, 0, "2 days used is within the cap of 3");
}

#[test]
fn the_cost_formula_reports_a_violated_week() {
    // 1 block/day, 5 active days, one Room: 3 required Sessions of the same
    // Offering can occupy at most one block per day, so they MUST land on 3
    // distinct days — above a cap of 2.
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        groups: vec![testing::group("G", None)],
        offerings: vec![offering("O", 3)],
        constraints: capped_days(2),
        ..ProblemSpec::new(testing::grid_5day(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(2_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0, "a HARD cap must not dead-end construction");
    assert_eq!(outcome.objective.max_days_violations, 1);
}

#[test]
fn a_run_within_the_cap_is_not_reported() {
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        groups: vec![testing::group("G", None)],
        offerings: vec![offering("O", 2)],
        constraints: capped_consecutive_days(3),
        ..ProblemSpec::new(testing::grid_5day(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(outcome.objective.max_consecutive_days_violations, 0);
}

#[test]
fn the_cost_formula_reports_a_violated_consecutive_run() {
    // Only 3 active days (Mon-Wed, already consecutive) and 3 required
    // Sessions with one Room and one block/day: every one of the 3 active
    // days MUST be used, so the run is forced to be exactly those 3 days —
    // above a cap of 2 — regardless of which days the search would
    // otherwise prefer.
    let grid =
        calendry_solver_core::slots::SlotTable::build(1, &[1, 2, 3], &testing::teaching_weeks(1))
            .unwrap();
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        groups: vec![testing::group("G", None)],
        offerings: vec![offering("O", 3)],
        constraints: capped_consecutive_days(2),
        ..ProblemSpec::new(grid)
    });

    let outcome = solve(&problem, SEED, moves(2_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(outcome.objective.max_consecutive_days_violations, 1);
}

#[test]
fn no_instance_configured_reports_nothing() {
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        groups: vec![testing::group("G", None)],
        offerings: vec![offering("O", 5)],
        constraints: testing::structural_room_only(),
        ..ProblemSpec::new(testing::grid_5day(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.max_days_violations, 0);
    assert_eq!(outcome.objective.max_consecutive_days_violations, 0);
}

#[test]
fn a_hard_cap_does_not_dead_end_construction() {
    // 5 required Sessions, one Room, one block/day: they MUST spread across
    // all 5 active days — impossible to satisfy a cap of 1. ADR-0025's
    // stance says this must still place every Session and merely report
    // the violation, never refuse to solve.
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        groups: vec![testing::group("G", None)],
        offerings: vec![offering("O", 5)],
        constraints: capped_days(1),
        ..ProblemSpec::new(testing::grid_5day(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(2_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0, "a hard day cap must not dead-end construction");
    assert!(outcome.objective.max_days_violations > 0);
}

#[test]
fn incremental_objective_matches_full_recomputation_with_max_days() {
    for seed in 0..8u64 {
        let spec = ProblemSpec {
            rooms: vec![testing::room("R0"), testing::room("R1")],
            groups: vec![testing::group("G", None)],
            offerings: vec![
                testing::with_groups(testing::offering("A", 4, &[0, 1]), &[0]),
                testing::with_groups(testing::offering("B", 3, &[0, 1]), &[0]),
            ],
            constraints: ConstraintSet {
                max_days: vec![MaxDaysInstance {
                    id: "c-days".into(),
                    kinds: vec![],
                    group: true,
                    person: false,
                    max_days: 2,
                }],
                max_consecutive_days: vec![MaxConsecutiveDaysInstance {
                    id: "c-consec".into(),
                    kinds: vec![],
                    group: true,
                    person: false,
                    max_consecutive_days: 2,
                }],
                ..testing::structural_room_only()
            },
            ..ProblemSpec::new(testing::grid_5day(1, 3))
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
