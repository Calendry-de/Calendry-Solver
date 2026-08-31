//! `TravelTimeBetweenRooms`: require a minimum gap when a Group's or
//! Person's consecutive same-day placements are in Rooms whose
//! `Room.location` differs — modeling travel between buildings/campuses.
//! Priced against the ACTUAL gap (`GridTime.gap_after`, reused from #26's
//! `MinimizeBreakSpanning`), not block adjacency alone.

use calendry_solver_core::aggregates::TravelTimeInstance;
use calendry_solver_core::ids::{PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec, Room};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::slots::GridTime;
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn room_at(id: &str, location: &str) -> Room {
    Room { location: location.to_string(), ..testing::room(id) }
}

/// 3 blocks x 60min, day starts 8:00, a 10-minute default gap between every
/// pair of consecutive blocks.
fn grid_time_with_10min_gap() -> GridTime {
    GridTime::new(60, 480, 10, vec![])
}

fn with_person_attendee(
    mut o: calendry_solver_core::problem::OfferingSpec,
    person: u32,
) -> calendry_solver_core::problem::OfferingSpec {
    o.participants = vec![PersonIdx(person)];
    o
}

fn person_rule(weight: f64, min_minutes: u32) -> ConstraintSet {
    ConstraintSet {
        travel_time_between_rooms: vec![TravelTimeInstance {
            id: "c-travel".into(),
            kinds: vec![],
            weight,
            group: false,
            person: true,
            min_minutes_between_sites: min_minutes,
        }],
        ..testing::structural_room_only()
    }
}

fn group_rule(weight: f64, min_minutes: u32) -> ConstraintSet {
    ConstraintSet {
        travel_time_between_rooms: vec![TravelTimeInstance {
            id: "c-travel".into(),
            kinds: vec![],
            weight,
            group: true,
            person: false,
            min_minutes_between_sites: min_minutes,
        }],
        ..testing::structural_room_only()
    }
}

#[test]
fn the_cost_formula_reports_a_violated_adjacent_block_crossing() {
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0, 1]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_at("A", "Building A"), room_at("B", "Building B")],
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: person_rule(5.0, 15), // 15 required, 10 actual
        grid_time: grid_time_with_10min_gap(),
        ..ProblemSpec::new(testing::grid(3, 1))
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.travel_cost, 5.0, "10 minutes actual is below the 15-minute requirement");
}

#[test]
fn a_gap_within_the_requirement_costs_nothing() {
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0, 1]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_at("A", "Building A"), room_at("B", "Building B")],
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: person_rule(5.0, 5), // 5 required, 10 actual: satisfied
        grid_time: grid_time_with_10min_gap(),
        ..ProblemSpec::new(testing::grid(3, 1))
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.travel_cost, 0.0);
}

#[test]
fn co_located_rooms_are_never_charged_even_with_no_gap() {
    // Both Rooms share the same `location`: no travel is implied, however
    // short the gap.
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0, 1]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_at("A", "Building A"), room_at("B", "Building A")],
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: person_rule(5.0, 15),
        grid_time: grid_time_with_10min_gap(),
        ..ProblemSpec::new(testing::grid(3, 1))
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.travel_cost, 0.0);
}

#[test]
fn non_adjacent_blocks_are_never_compared() {
    // Blocks 0 and 2 are not adjacent (block 1 sits between them): no
    // travel-time pair exists between them regardless of Room.
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0, 1]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_at("A", "Building A"), room_at("B", "Building B")],
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: person_rule(5.0, 999),
        grid_time: grid_time_with_10min_gap(),
        ..ProblemSpec::new(testing::grid(3, 1))
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(2), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.travel_cost, 0.0);
}

#[test]
fn the_group_axis_reads_independently_of_the_person_axis() {
    let offerings = (0..2)
        .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 1, &[0, 1]), &[0]))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_at("A", "Building A"), room_at("B", "Building B")],
        groups: vec![testing::group("G", None)],
        offerings,
        constraints: group_rule(5.0, 15),
        grid_time: grid_time_with_10min_gap(),
        ..ProblemSpec::new(testing::grid(3, 1))
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.travel_cost, 5.0);
}

#[test]
fn no_instance_configured_costs_nothing() {
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0, 1]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_at("A", "Building A"), room_at("B", "Building B")],
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: testing::structural_room_only(),
        grid_time: grid_time_with_10min_gap(),
        ..ProblemSpec::new(testing::grid(3, 1))
    });

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.travel_cost, 0.0);
}

#[test]
fn the_search_prefers_the_co_located_room_when_available() {
    // Two adjacent-block Sessions for the same Person; a third Room shares
    // Building A with the first, so the search should prefer it over the
    // cross-campus Building B Room when nothing else distinguishes them.
    let offerings = (0..2)
        .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0, 1, 2]), 0))
        .collect();
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![
            room_at("A", "Building A"),
            room_at("B", "Building B"),
            room_at("C", "Building A"),
        ],
        persons: vec![testing::person("P", &[])],
        offerings,
        constraints: person_rule(5.0, 15),
        grid_time: grid_time_with_10min_gap(),
        ..ProblemSpec::new(testing::grid(3, 1))
    });

    let outcome = solve(&problem, SEED, moves(2_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.travel_cost, 0.0,
        "a co-located pairing exists (Room A + Room C, both Building A) and must win"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_travel_time() {
    for seed in 0..8u64 {
        let offerings: Vec<_> = (0..4)
            .map(|i| with_person_attendee(testing::offering(&format!("O{i}"), 1, &[0, 1]), 0))
            .collect();
        let spec = ProblemSpec {
            rooms: vec![room_at("A", "Building A"), room_at("B", "Building B")],
            persons: vec![testing::person("P", &[])],
            offerings,
            constraints: person_rule(5.0, 15),
            grid_time: grid_time_with_10min_gap(),
            ..ProblemSpec::new(testing::grid(6, 2))
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
