//! `MeetTogether` — two (or more) Offerings taught together, in one Room, at
//! one time (issue #55). The one relation kind built as a true occupancy
//! FILTER rather than priced (unlike `SameTime`/`SameDays`/`SameStart`):
//! whichever member is placed first in a week establishes that week's
//! (start, room); every other member's Session that week is then restricted
//! to EXACTLY that cell, and the members occupy the exclusive Room TOGETHER
//! instead of clashing on it — subject to their SUMMED `min_capacity`
//! against `Room.capacity`.
//!
//! This is the one relation that also touches `RoomDoubleBooking` itself: a
//! legitimate share must never be reported as a clash by the independent
//! structural checker (ADR-0014), so that exemption gets its own coverage
//! here too, alongside a genuine disagreement in already-placed input.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::ids::{OfferingIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{Problem, ProblemSpec, RelationKind, RelationSpec, Room};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::solution::SearchState;
use calendry_solver_core::testing::{
    fixture, grid, offering, room, structural_room_only, with_min_capacity,
};
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn room_with_capacity(id: &str, capacity: u32) -> Room {
    Room { capacity, ..room(id) }
}

fn two_offerings_meeting_together(rooms: Vec<Room>, blocks: u32) -> Problem {
    let a = offering("a", 1, &(0..blocks).collect::<Vec<_>>());
    let b = offering("b", 1, &(0..blocks).collect::<Vec<_>>());
    let mut spec = ProblemSpec {
        rooms,
        offerings: vec![a, b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::MeetTogether,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid(blocks, 1), structural_room_only())
    };
    spec.expand_placements();
    Problem::build(spec).unwrap()
}

#[test]
fn a_second_member_may_join_the_first_members_exact_cell() {
    let problem = two_offerings_meeting_together(vec![room_with_capacity("r0", 100)], 2);
    let mut state = SearchState::from_fixed(&problem);
    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(
        state.can_place(&problem, PlacementIdx(1), Placement::single(SlotIdx(0), RoomIdx(0))),
        "a MeetTogether sibling must be free to join the exact cell its member already holds"
    );
}

#[test]
fn a_second_member_may_not_join_a_different_slot() {
    let problem = two_offerings_meeting_together(vec![room_with_capacity("r0", 100)], 2);
    let mut state = SearchState::from_fixed(&problem);
    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(
        !state.can_place(&problem, PlacementIdx(1), Placement::single(SlotIdx(1), RoomIdx(0))),
        "Same Time is forced: a mismatching slot must be rejected even though it is otherwise free"
    );
}

#[test]
fn a_second_member_may_not_join_a_different_room() {
    let problem = two_offerings_meeting_together(
        vec![room_with_capacity("r0", 100), room_with_capacity("r1", 100)],
        1,
    );
    let mut state = SearchState::from_fixed(&problem);
    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(
        !state.can_place(&problem, PlacementIdx(1), Placement::single(SlotIdx(0), RoomIdx(1))),
        "Same Room is forced: a different (but otherwise free) Room must be rejected"
    );
}

#[test]
fn an_unrelated_offering_may_not_take_the_shared_cell() {
    // A third Offering, no relation at all, must still be refused the
    // occupied cell — sharing is exclusive to the relation's own members.
    let a = offering("a", 1, &[0]);
    let b = offering("b", 1, &[0]);
    let c = offering("c", 1, &[0]);
    let mut spec = ProblemSpec {
        rooms: vec![room_with_capacity("r0", 100)],
        offerings: vec![a, b, c],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::MeetTogether,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid(1, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let mut state = SearchState::from_fixed(&problem);
    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(!state.can_place(&problem, PlacementIdx(2), Placement::single(SlotIdx(0), RoomIdx(0))));
}

#[test]
fn combined_capacity_over_the_room_cap_is_refused() {
    // Room seats 15; "a" alone (10) fits, "a" + "b" (10 + 10 = 20) does not.
    let a = with_min_capacity(offering("a", 1, &[0]), 10);
    let b = with_min_capacity(offering("b", 1, &[0]), 10);
    let mut spec = ProblemSpec {
        rooms: vec![room_with_capacity("r0", 15)],
        offerings: vec![a, b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::MeetTogether,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid(1, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let mut state = SearchState::from_fixed(&problem);
    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(
        !state.can_place(&problem, PlacementIdx(1), Placement::single(SlotIdx(0), RoomIdx(0))),
        "10 + 10 = 20 exceeds a 15-seat Room"
    );
}

#[test]
fn combined_capacity_within_the_room_cap_is_allowed() {
    let a = with_min_capacity(offering("a", 1, &[0]), 10);
    let b = with_min_capacity(offering("b", 1, &[0]), 5);
    let mut spec = ProblemSpec {
        rooms: vec![room_with_capacity("r0", 15)],
        offerings: vec![a, b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::MeetTogether,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid(1, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let mut state = SearchState::from_fixed(&problem);
    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(state.can_place(&problem, PlacementIdx(1), Placement::single(SlotIdx(0), RoomIdx(0))));
}

#[test]
fn an_unbounded_room_never_refuses_on_capacity() {
    // `Room.capacity == 0` means unbounded (issue #62's reading, reused here).
    let a = with_min_capacity(offering("a", 1, &[0]), 10_000);
    let b = with_min_capacity(offering("b", 1, &[0]), 10_000);
    let mut spec = ProblemSpec {
        rooms: vec![room_with_capacity("r0", 0)],
        offerings: vec![a, b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::MeetTogether,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid(1, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let mut state = SearchState::from_fixed(&problem);
    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(state.can_place(&problem, PlacementIdx(1), Placement::single(SlotIdx(0), RoomIdx(0))));
}

#[test]
fn releasing_the_last_member_frees_the_cell_for_anyone() {
    let problem = two_offerings_meeting_together(vec![room_with_capacity("r0", 100)], 1);
    let mut state = SearchState::from_fixed(&problem);
    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));
    assert!(state.place(&problem, PlacementIdx(1), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(state.unplace(&problem, PlacementIdx(1), Placement::single(SlotIdx(0), RoomIdx(0))));
    assert!(
        state.can_place(&problem, PlacementIdx(1), Placement::single(SlotIdx(0), RoomIdx(0))),
        "rejoining after leaving must still work"
    );

    assert!(state.unplace(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));
    // Both members gone: the cell must be entirely free again, for anyone.
    assert!(state.can_place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));
}

#[test]
fn two_unrelated_meet_together_relations_do_not_interfere() {
    // "a"/"b" share relation rel-1; "c"/"d" share a SEPARATE relation rel-2.
    // Both pairs happen to prefer the same (single) Room and slot, but must
    // never be allowed to merge into one four-way share.
    let a = offering("a", 1, &[0]);
    let b = offering("b", 1, &[0]);
    let c = offering("c", 1, &[0]);
    let d = offering("d", 1, &[0]);
    let mut spec = ProblemSpec {
        rooms: vec![room_with_capacity("r0", 100)],
        offerings: vec![a, b, c, d],
        relations: vec![
            RelationSpec {
                id: "rel-1".to_string(),
                kind: RelationKind::MeetTogether,
                members: vec![OfferingIdx(0), OfferingIdx(1)],
            },
            RelationSpec {
                id: "rel-2".to_string(),
                kind: RelationKind::MeetTogether,
                members: vec![OfferingIdx(2), OfferingIdx(3)],
            },
        ],
        ..fixture(grid(1, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let mut state = SearchState::from_fixed(&problem);
    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));
    assert!(
        !state.can_place(&problem, PlacementIdx(2), Placement::single(SlotIdx(0), RoomIdx(0))),
        "rel-2's member must not be able to join rel-1's cell"
    );
}

#[test]
fn the_search_places_both_members_of_a_meet_together_pair() {
    let problem = two_offerings_meeting_together(
        vec![room_with_capacity("r0", 100), room_with_capacity("r1", 100)],
        3,
    );
    let outcome = solve(&problem, SEED, moves(2_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);

    let a = outcome
        .solution
        .get(problem.placement_ids().next().unwrap())
        .unwrap();
    let b = outcome
        .solution
        .get(problem.placement_ids().nth(1).unwrap())
        .unwrap();
    assert_eq!(a.start, b.start, "MeetTogether members must land in the same slot");
    assert_eq!(a.room, b.room, "MeetTogether members must land in the same Room");
}

// ---------------------------------------------------------------------------
// The RoomDoubleBooking exemption, and the independent structural check.
// ---------------------------------------------------------------------------

#[test]
fn evaluate_hard_does_not_report_a_legitimate_share_as_room_double_booking() {
    let problem = two_offerings_meeting_together(vec![room_with_capacity("r0", 100)], 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(0), RoomIdx(0))));

    let violations = evaluate_hard(&problem, &solution);
    assert!(
        !violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::RoomDoubleBooking),
        "a MeetTogether pair sharing a Room legitimately must never be reported as a clash, got {violations:?}"
    );
}

#[test]
fn evaluate_hard_still_reports_an_unrelated_pair_sharing_a_room() {
    // Baseline: the exemption must be scoped to an ACTUAL shared relation,
    // not to Room-sharing in general.
    let a = offering("a", 1, &[0]);
    let b = offering("b", 1, &[0]);
    let mut spec = ProblemSpec {
        rooms: vec![room_with_capacity("r0", 100)],
        offerings: vec![a, b],
        ..fixture(grid(1, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(0), RoomIdx(0))));

    let violations = evaluate_hard(&problem, &solution);
    assert!(
        violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::RoomDoubleBooking)
    );
}

#[test]
fn evaluate_hard_reports_a_meet_together_pair_that_disagrees() {
    // Already-placed input the search never had a chance to avoid: two
    // related Offerings NOT sharing a cell.
    let problem = two_offerings_meeting_together(
        vec![room_with_capacity("r0", 100), room_with_capacity("r1", 100)],
        2,
    );
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(1), RoomIdx(1))));

    let violations = evaluate_hard(&problem, &solution);
    assert!(
        violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::MeetTogetherRelation
                && v.constraint_id == "rel-1"),
        "expected a MeetTogetherRelation violation, got {violations:?}"
    );
}

#[test]
fn evaluate_hard_reports_a_meet_together_pair_over_combined_capacity() {
    let a = with_min_capacity(offering("a", 1, &[0]), 10);
    let b = with_min_capacity(offering("b", 1, &[0]), 10);
    let mut spec = ProblemSpec {
        rooms: vec![room_with_capacity("r0", 15)],
        offerings: vec![a, b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::MeetTogether,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid(1, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    // Bypasses the search's own filter on purpose, the same way every other
    // "already-bad LOCKED input" test in this codebase does: construct the
    // Solution by hand rather than through `SearchState`.
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(0), RoomIdx(0))));

    let violations = evaluate_hard(&problem, &solution);
    assert!(
        violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::MeetTogetherRelation
                && v.constraint_id == "rel-1"),
        "expected a MeetTogetherRelation capacity violation, got {violations:?}"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_meet_together() {
    for seed in 0..8u64 {
        let a = offering("a", 3, &[0, 1]);
        let b = offering("b", 3, &[0, 1]);
        let mut spec = ProblemSpec {
            rooms: vec![room_with_capacity("r0", 100), room_with_capacity("r1", 100)],
            offerings: vec![a, b],
            relations: vec![RelationSpec {
                id: "rel-1".to_string(),
                kind: RelationKind::MeetTogether,
                members: vec![OfferingIdx(0), OfferingIdx(1)],
            }],
            ..fixture(grid(2, 3), structural_room_only())
        };
        spec.expand_placements();
        let problem = Problem::build(spec).unwrap();
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
