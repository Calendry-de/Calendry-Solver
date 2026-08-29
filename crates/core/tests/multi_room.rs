//! `Offering.required_room_count` and `eligible_room_combinations`: a Session
//! occupying more than one Room at once.
//!
//! Combinatorial enumeration and capacity summing live in
//! `convert::build_offerings` (service crate, exercised in
//! `crates/service/tests/multi_room.rs`) — these exercise the solver's
//! placement primitive itself: marking, checking and costing every Room in a
//! combination together, not just the primary one.

use calendry_solver_core::ids::{PlacementIdx, RoomIdx};
use calendry_solver_core::problem::ProblemSpec;
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::soft::SoftParams;
use calendry_solver_core::testing;

mod common;
use common::{SEED, moves};

#[test]
fn a_multi_room_offering_places_using_every_room_in_its_combination() {
    // Rooms 0 and 1 form the only valid combination; Room 2 is never
    // eligible. One slot only, so there is exactly one placement to make.
    let offering = testing::with_room_combinations(testing::offering("O", 1, &[]), 2, &[0, 1]);
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(3),
        offerings: vec![offering],
        constraints: testing::structural_room_only(),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);

    let pl = outcome
        .solution
        .get(PlacementIdx(0))
        .expect("the one Session must be placed");
    let rooms: Vec<RoomIdx> = pl.all_rooms().collect();
    assert_eq!(rooms.len(), 2, "both Rooms of the combination must be occupied at once");
    assert!(rooms.contains(&RoomIdx(0)));
    assert!(rooms.contains(&RoomIdx(1)));
}

#[test]
fn only_one_of_two_needed_rooms_free_still_leaves_it_unplaced() {
    // One slot only. Room 1 is pre-occupied by an unrelated immovable
    // Session; Room 0 stays free. The only valid combination is (0, 1), so
    // the Offering must come up unplaced rather than "half-satisfied" by
    // Room 0 alone.
    let offering = testing::with_room_combinations(testing::offering("O", 1, &[]), 2, &[0, 1]);
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        offerings: vec![offering],
        fixed: vec![testing::fixed_session("blocker", Some(1), 0)],
        constraints: testing::structural_room_only(),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(
        outcome.objective.unplaced, 1,
        "Room 1 is busy, so the only combination cannot fit even though Room 0 is free"
    );
}

#[test]
fn two_offerings_still_collide_when_they_share_only_a_secondary_room() {
    // Offering A's only combination is (0, 1); Offering B's is (1, 2) — they
    // share Room 1 as a SECONDARY Room for one of them, never as either's
    // primary. One slot only, so both cannot be placed at once. A `mark`/
    // `is_free` that only ever consulted the primary Room would see primaries
    // 0 and 2, find no clash, and incorrectly place both.
    let a = testing::with_room_combinations(testing::offering("A", 1, &[]), 2, &[0, 1]);
    let b = testing::with_room_combinations(testing::offering("B", 1, &[]), 2, &[1, 2]);
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(3),
        offerings: vec![a, b],
        constraints: testing::structural_room_only(),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(
        outcome.objective.unplaced, 1,
        "both Offerings need Room 1 in the only slot available; exactly one must lose out"
    );
}

#[test]
fn a_combinations_soft_cost_is_the_sum_of_its_rooms_costs() {
    // Room A physical, Room B virtual. MinimizeOnline charges `weight` for
    // every VIRTUAL Room a Session occupies — a Session split across one of
    // each must be charged for the one that is virtual: not zero (as reading
    // only a physical primary would give) and not double (as double-counting
    // would give if summing were broken the other way).
    let offering = testing::with_room_combinations(testing::offering("O", 1, &[]), 2, &[0, 1]);
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![
            testing::room_with("A", 1, false),
            testing::room_with("B", 1, true),
        ],
        offerings: vec![offering],
        constraints: testing::with_soft(vec![testing::soft(
            "c-online",
            4.0,
            SoftParams::MinimizeOnline,
        )]),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(outcome.objective.soft, 4.0, "exactly one of the two Rooms is virtual");
}

#[test]
fn incremental_objective_matches_full_recomputation_with_multi_room_offerings() {
    // Several multi-Room Offerings contending over an overlapping Room pool
    // and several slots, under LNS: `Trial::place`/`unplace` maintain `soft`
    // incrementally by summing over `all_rooms()`, and this must never drift
    // from a from-scratch recomputation doing the same sum.
    for seed in 0..8u64 {
        let a = testing::with_room_combinations(testing::offering("A", 2, &[]), 2, &[0, 1, 2]);
        let b = testing::with_room_combinations(testing::offering("B", 2, &[]), 3, &[0, 1, 2, 3]);
        let problem = testing::assemble(ProblemSpec {
            rooms: vec![
                testing::room_with("R0", 1, false),
                testing::room_with("R1", 3, true),
                testing::room_with("R2", 5, false),
                testing::room_with("R3", 2, false),
            ],
            offerings: vec![a, b],
            constraints: testing::with_soft(vec![
                testing::soft("c-online", 4.0, SoftParams::MinimizeOnline),
                testing::soft(
                    "c-rank",
                    3.0,
                    SoftParams::MinimizeRoomRank { rank_threshold: 2, invert: false },
                ),
            ]),
            ..ProblemSpec::new(testing::grid(2, 3))
        });

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
