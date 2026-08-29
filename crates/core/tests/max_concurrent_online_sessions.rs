//! `MaxConcurrentOnlineSessions`: a tenant-wide cap on how many Sessions may
//! be online at the same slot, independent of Group or kind.
//!
//! Filterable, unlike `MaxOnlineShare` — the count at one slot has no moving
//! denominator, so `Occupancy::is_free` enforces it directly and the search
//! can never create a violation once it is configured. `evaluate_hard` is
//! still exercised independently, against a hand-built `Solution`, the same
//! way slice 4's own tests check an authoritative rule the search itself
//! cannot violate.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, MaxConcurrentOnlineInstance, ProblemSpec};
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_core::testing;
use calendry_solver_core::{Placement, Solution};

mod common;
use common::SEED;

fn budget() -> Budget {
    Budget { max_wall_millis: 0, max_moves: 5_000 }
}

fn capped(cap: u32) -> ConstraintSet {
    ConstraintSet {
        max_concurrent_online_sessions: vec![MaxConcurrentOnlineInstance {
            id: "c-cap".into(),
            max_concurrent: cap,
        }],
        ..testing::structural_room_only()
    }
}

/// 3 Offerings, one Session each, only virtual Rooms eligible, one slot.
fn three_online_offerings_one_slot(cap: u32, n_virtual_rooms: u32) -> ProblemSpec {
    let rooms: Vec<_> = (0..n_virtual_rooms)
        .map(|i| testing::room_with(&format!("V{i}"), 1, true))
        .collect();
    let eligible: Vec<u32> = (0..n_virtual_rooms).collect();
    ProblemSpec {
        rooms,
        offerings: vec![
            testing::offering("A", 1, &eligible),
            testing::offering("B", 1, &eligible),
            testing::offering("C", 1, &eligible),
        ],
        constraints: capped(cap),
        ..ProblemSpec::new(testing::grid(1, 1))
    }
}

#[test]
fn the_search_never_exceeds_the_cap() {
    // 3 virtual Rooms exist — plenty of ROOM eligibility — but the cap of 2
    // must still leave one of the three Offerings unplaced, since no
    // non-virtual Room is eligible for any of them.
    let problem = testing::assemble(three_online_offerings_one_slot(2, 3));
    let outcome = solve(&problem, SEED, budget(), &NeverHalt);

    assert_eq!(outcome.objective.unplaced, 1, "the cap allows only 2 of the 3 online Sessions");
    // The one unplaced Offering legitimately reports ExactFrequency; the cap
    // itself must never be the thing the search violates.
    let violations = evaluate_hard(&problem, &outcome.solution);
    assert!(
        !violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::MaxConcurrentOnlineSessions),
        "the search must never itself create a cap violation: {violations:?}"
    );
}

#[test]
fn not_configured_means_no_cap() {
    let mut spec = three_online_offerings_one_slot(2, 3);
    spec.constraints = testing::structural_room_only();
    let problem = testing::assemble(spec);
    let outcome = solve(&problem, SEED, budget(), &NeverHalt);

    assert_eq!(outcome.objective.unplaced, 0, "no cap configured, all 3 fit online at once");
}

#[test]
fn evaluate_hard_reports_a_slot_already_over_the_cap() {
    // Bypasses the search entirely: 3 Sessions hand-placed online in the same
    // slot, cap 2, cannot arise from the search itself but must still be
    // reported if the caller's own data (e.g. 3 locked Sessions) produces it.
    let spec = ProblemSpec {
        rooms: vec![
            testing::room_with("V0", 1, true),
            testing::room_with("V1", 1, true),
            testing::room_with("V2", 1, true),
        ],
        offerings: vec![
            testing::offering("A", 1, &[0]),
            testing::offering("B", 1, &[1]),
            testing::offering("C", 1, &[2]),
        ],
        constraints: capped(2),
        ..ProblemSpec::new(testing::grid(1, 1))
    };
    let problem = testing::assemble(spec);

    let mut solution = Solution::empty(&problem);
    let slot = SlotIdx(0);
    solution.set(PlacementIdx(0), Some(Placement::single(slot, RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(slot, RoomIdx(1))));
    solution.set(PlacementIdx(2), Some(Placement::single(slot, RoomIdx(2))));

    let violations = evaluate_hard(&problem, &solution);
    assert!(
        violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::MaxConcurrentOnlineSessions),
        "{violations:?}"
    );
}
