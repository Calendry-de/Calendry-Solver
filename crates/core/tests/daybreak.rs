//! `Daybreak`: minimum wall-clock rest between one teaching day's last
//! occupied block and the next teaching day's first, per Group and/or
//! Person — the one welfare constraint no day-bounded rule (`Compactness`,
//! `MaxDailySpan`, `MaxConsecutiveBlocks`) can approximate, because they all
//! stop at midnight.
//!
//! SOFT, priced like `RoomTurnaroundBuffer` — a minimum-gap requirement,
//! not a cap, so the "tightest" convention is `max`, not `min`, unlike
//! every capped type in this catalogue.

use calendry_solver_core::aggregates::DaybreakInstance;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::slots::{GridTime, SlotTable};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

/// `Daybreak`'s Person axis reads `Offering.attendees` — participants plus
/// Group members, the same substrate `Compactness`'s Person axis reads —
/// NOT `Offering.lecturers`, which is a separate list nothing here touches.
/// A direct participant, not `testing::with_lecturers`, is what puts a
/// Person on that axis.
fn with_person_attendee(
    mut o: calendry_solver_core::problem::OfferingSpec,
    person: u32,
) -> calendry_solver_core::problem::OfferingSpec {
    o.participants = vec![calendry_solver_core::ids::PersonIdx(person)];
    o
}

/// 10 blocks x 90min, day starts 7:00 (420), no gaps: a day spans 7:00 to
/// 22:00 (1320). The overnight rest between a Monday ending at block 9 and
/// a Tuesday starting at block 0 is `(1440-1320)+420 = 540` minutes (9h).
fn evening_to_morning_grid() -> (SlotTable, GridTime) {
    (
        SlotTable::build(10, &[1, 2], &testing::teaching_weeks(1)).unwrap(),
        GridTime::new(90, 420, 0, vec![]),
    )
}

fn person_rule(weight: f64, min_rest_minutes: u32) -> ConstraintSet {
    ConstraintSet {
        daybreak: vec![DaybreakInstance {
            id: "c-daybreak".into(),
            kinds: vec![],
            weight,
            group: false,
            person: true,
            min_rest_minutes,
        }],
        ..testing::structural_room_only()
    }
}

fn group_rule(weight: f64, min_rest_minutes: u32) -> ConstraintSet {
    ConstraintSet {
        daybreak: vec![DaybreakInstance {
            id: "c-daybreak".into(),
            kinds: vec![],
            weight,
            group: true,
            person: false,
            min_rest_minutes,
        }],
        ..testing::structural_room_only()
    }
}

#[test]
fn the_cost_formula_reports_a_violated_overnight_gap_person_axis() {
    let (grid, grid_time) = evening_to_morning_grid();
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: person_rule(5.0, 600), // 10h required, 9h actual
        grid_time,
        ..ProblemSpec::new(grid)
    });

    let mut solution = Solution::empty(&problem);
    // Monday, last block (9): ends at 22:00.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(9), RoomIdx(0))));
    // Tuesday, first block (0): starts at 7:00.
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(10), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.daybreak_cost, 5.0, "9h actual rest is below the 10h requirement");
}

#[test]
fn a_rest_period_within_the_requirement_costs_nothing() {
    let (grid, grid_time) = evening_to_morning_grid();
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: person_rule(5.0, 480), // 8h required, 9h actual: satisfied
        grid_time,
        ..ProblemSpec::new(grid)
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(9), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(10), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.daybreak_cost, 0.0);
}

#[test]
fn a_gap_in_the_middle_of_the_day_is_not_an_overnight_pair() {
    // Both Sessions on the SAME day (Monday): not a consecutive-DAY pair at
    // all, however far apart the blocks are within it.
    let (grid, grid_time) = evening_to_morning_grid();
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: person_rule(5.0, 600),
        grid_time,
        ..ProblemSpec::new(grid)
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(9), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.daybreak_cost, 0.0);
}

#[test]
fn the_group_axis_reads_independently_of_the_person_axis() {
    let (grid, grid_time) = evening_to_morning_grid();
    let offerings = (0..2)
        .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 1, &[0]), &[0]))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![testing::group("G", None)],
        offerings,
        constraints: group_rule(5.0, 600),
        grid_time,
        ..ProblemSpec::new(grid)
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(9), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(10), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.daybreak_cost, 5.0);
}

#[test]
fn no_instance_configured_costs_nothing() {
    let (grid, grid_time) = evening_to_morning_grid();
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: testing::structural_room_only(),
        grid_time,
        ..ProblemSpec::new(grid)
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(9), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(10), RoomIdx(0))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.daybreak_cost, 0.0);
}

#[test]
fn the_search_prefers_the_rest_satisfying_layout() {
    // A Person has two single-block Sessions across a 2-day, 10-block-per-day
    // grid: nothing else distinguishes candidate slots, so the search must
    // avoid ending Monday at block 9 while starting Tuesday at block 0 (9h
    // rest, violating a 10h requirement) when free slots exist that would
    // give it more.
    let (grid, grid_time) = evening_to_morning_grid();
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: person_rule(5.0, 600),
        grid_time,
        ..ProblemSpec::new(grid)
    });

    let outcome = solve(&problem, SEED, moves(2_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.daybreak_cost, 0.0,
        "a rest-satisfying layout exists (e.g. both on the same day, or not at the day's edges) \
         and must win"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_daybreak() {
    for seed in 0..8u64 {
        let (grid, grid_time) = evening_to_morning_grid();
        let offerings: Vec<_> = (0..4)
            .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0]), 0))
            .collect();
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            persons: vec![testing::person("P", &[])],
            offerings,
            constraints: person_rule(5.0, 600),
            grid_time,
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
