//! `LecturerConsistency`: once a lecturer holds one Session of a recurring
//! Offering, they should hold the rest of it too — the lecturer-axis
//! counterpart of `RoomConsistency`, but only ever live for a genuine
//! lecturer-pool Offering (`Offering::has_lecturer_pool`): a fixed
//! assignment's distinct lecturer count never changes, so this type can
//! never fire for it.
//!
//! An aggregate over an entire Offering's Sessions across the WHOLE TERM,
//! keyed by Offering, unbounded by day or window — the same new shape
//! `RoomConsistency` uses for the Room axis.
//! Cost: `max(0, distinct_lecturers - required_lecturer_count)`.

use calendry_solver_core::aggregates::LecturerConsistencyInstance;
use calendry_solver_core::ids::{PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::solution::MAX_LECTURERS;
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn consistency_rule(weight: f64) -> LecturerConsistencyInstance {
    LecturerConsistencyInstance { id: "c-lecturer-consistency".into(), kinds: vec![], weight }
}

fn with_lecturer(base: Placement, person: u32) -> Placement {
    let mut lecturers = [None; MAX_LECTURERS];
    lecturers[0] = Some(PersonIdx(person));
    Placement { lecturers, ..base }
}

/// One Offering with a genuine 3-candidate pool needing 1, 3 required
/// Sessions, 1 Room, 3 slots — plenty of room to place all three Sessions
/// with no structural pressure to pick any particular candidate.
fn one_offering_three_sessions(weight: f64) -> calendry_solver_core::Problem {
    let o = testing::with_lecturer_pool(testing::offering("O", 3, &[0]), 1, &[0, 1, 2]);
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![
            testing::person("p0", &[]),
            testing::person("p1", &[]),
            testing::person("p2", &[]),
        ],
        offerings: vec![o],
        constraints: ConstraintSet {
            lecturer_consistency: vec![consistency_rule(weight)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(3, 1))
    })
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let problem = one_offering_three_sessions(5.0);
    let mut solution = Solution::empty(&problem);
    // 2 Sessions with p0, 1 with p1: distinct = 2, required = 1,
    // excess = 2 - 1 = 1, cost = weight * excess = 5 * 1 = 5.
    solution
        .set(PlacementIdx(0), Some(with_lecturer(Placement::single(SlotIdx(0), RoomIdx(0)), 0)));
    solution
        .set(PlacementIdx(1), Some(with_lecturer(Placement::single(SlotIdx(1), RoomIdx(0)), 0)));
    solution
        .set(PlacementIdx(2), Some(with_lecturer(Placement::single(SlotIdx(2), RoomIdx(0)), 1)));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.lecturer_consistency_cost, 5.0);
}

#[test]
fn every_session_with_the_same_lecturer_costs_nothing() {
    let problem = one_offering_three_sessions(5.0);
    let mut solution = Solution::empty(&problem);
    solution
        .set(PlacementIdx(0), Some(with_lecturer(Placement::single(SlotIdx(0), RoomIdx(0)), 0)));
    solution
        .set(PlacementIdx(1), Some(with_lecturer(Placement::single(SlotIdx(1), RoomIdx(0)), 0)));
    solution
        .set(PlacementIdx(2), Some(with_lecturer(Placement::single(SlotIdx(2), RoomIdx(0)), 0)));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.lecturer_consistency_cost, 0.0);
}

#[test]
fn a_zero_weight_charges_nothing_regardless_of_lecturer_variety() {
    let problem = one_offering_three_sessions(0.0);
    let mut solution = Solution::empty(&problem);
    solution
        .set(PlacementIdx(0), Some(with_lecturer(Placement::single(SlotIdx(0), RoomIdx(0)), 0)));
    solution
        .set(PlacementIdx(1), Some(with_lecturer(Placement::single(SlotIdx(1), RoomIdx(0)), 1)));
    solution
        .set(PlacementIdx(2), Some(with_lecturer(Placement::single(SlotIdx(2), RoomIdx(0)), 2)));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.lecturer_consistency_cost, 0.0);
}

#[test]
fn a_fixed_assignment_never_charges_regardless_of_weight() {
    // No pool: `with_lecturers` sets a single fixed lecturer, so every
    // Session shares it by construction and this type can never fire.
    let o = testing::with_lecturers(testing::offering("O", 3, &[0]), &[0]);
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person("p0", &[])],
        offerings: vec![o],
        constraints: ConstraintSet {
            lecturer_consistency: vec![consistency_rule(10.0)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(3, 1))
    });
    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.lecturer_consistency_cost, 0.0);
}

#[test]
fn the_search_keeps_an_offerings_sessions_with_one_lecturer() {
    let problem = one_offering_three_sessions(10.0);
    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.lecturer_consistency_cost, 0.0,
        "3 interchangeable candidates and no other pressure always allow one lecturer for all 3 Sessions"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_lecturer_consistency() {
    for seed in 0..8u64 {
        let a = testing::with_lecturer_pool(testing::offering("A", 3, &[0, 1, 2]), 1, &[0, 1, 2]);
        let b = testing::with_lecturer_pool(testing::offering("B", 2, &[0, 1, 2]), 2, &[0, 1, 2]);
        let spec = ProblemSpec {
            rooms: testing::rooms(3),
            persons: vec![
                testing::person("p0", &[]),
                testing::person("p1", &[]),
                testing::person("p2", &[]),
            ],
            offerings: vec![a, b],
            constraints: ConstraintSet {
                lecturer_consistency: vec![consistency_rule(4.0)],
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
