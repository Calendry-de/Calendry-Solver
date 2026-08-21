//! Slice 2 acceptance tests: the three remaining structural constraints and the
//! nested-group closure.
//!
//! These are written to be falsifiable against the *wrong* implementations, not
//! merely green against the right one. In particular
//! `siblings_may_meet_simultaneously` fails if anyone "simplifies" the closure
//! to symmetric expansion, and `cross_tree_person_clash_is_invisible_to_groups`
//! demonstrates the clash is real before showing the person check catches it.

use calendry_solver_core::constraints::{
    GROUP_DOUBLE_BOOKING, LECTURER_DOUBLE_BOOKING, PERSON_DOUBLE_BOOKING, ROOM_DOUBLE_BOOKING,
    evaluate_hard,
};
use calendry_solver_core::ids::PlacementIdx;
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_core::solution::Solution;
use calendry_solver_core::{Problem, SolveOutcome, testing};

const SEED: u64 = 0xC0FFEE;

fn run(problem: &Problem) -> SolveOutcome {
    solve(problem, SEED, Budget::default(), &NeverHalt)
}

fn slot_of(o: &SolveOutcome, i: u32) -> u32 {
    o.solution
        .get(PlacementIdx(i))
        .expect("placement should have been placed")
        .start
        .0
}

// ---------------------------------------------------------------------------
// Test 1 — the sibling regression test
// ---------------------------------------------------------------------------

#[test]
fn siblings_may_meet_simultaneously() {
    // Classes B and C both sit under cohort A. One slot, two rooms.
    let problem = testing::sibling_classes();
    let outcome = run(&problem);

    assert!(
        outcome.hard_violations.is_empty(),
        "two sibling classes meeting at once is the normal case, got {:?}",
        outcome.hard_violations
    );

    // Both placed, and necessarily in the single available slot.
    assert_eq!(outcome.solution.placed_count(), 2);
    assert_eq!(slot_of(&outcome, 0), 0);
    assert_eq!(slot_of(&outcome, 1), 0);

    // Different rooms, since RoomDoubleBooking still applies.
    let a = outcome.solution.get(PlacementIdx(0)).unwrap().room;
    let b = outcome.solution.get(PlacementIdx(1)).unwrap().room;
    assert_ne!(a, b);
}

// ---------------------------------------------------------------------------
// Test 2 — parent/child conflict, both directions
// ---------------------------------------------------------------------------

#[test]
fn a_cohort_session_blocks_its_child_class() {
    // Cohort A is pinned at slot 0; a session for class B must go elsewhere.
    let problem = testing::parent_child_conflict(true);
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(
        slot_of(&outcome, 0),
        1,
        "child must not share a slot with its pinned ancestor"
    );
}

#[test]
fn a_class_session_blocks_its_parent_cohort() {
    // The reverse direction: class B pinned, cohort A needs placing.
    let problem = testing::parent_child_conflict(false);
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(
        slot_of(&outcome, 0),
        1,
        "ancestor must not share a slot with its pinned descendant"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — depth
// ---------------------------------------------------------------------------

#[test]
fn closure_is_transitive_over_a_deep_chain() {
    // L0 <- L1 <- L2 <- L3, root pinned at slot 0, leaf needs placing.
    let problem = testing::deep_chain();
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(
        slot_of(&outcome, 0),
        1,
        "a 3-hop ancestor must still block the leaf"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — lecturer double-booking
// ---------------------------------------------------------------------------

#[test]
fn one_lecturer_cannot_lead_two_sessions_at_once() {
    let problem = testing::lecturer_clash();
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(outcome.solution.placed_count(), 2);
    assert_ne!(
        slot_of(&outcome, 0),
        slot_of(&outcome, 1),
        "the shared lecturer must force different slots"
    );
}

#[test]
fn lecturer_clash_is_reported_when_the_input_already_contains_one() {
    // Force the clash by shrinking the grid to a single slot: both sessions
    // must land there, so the violation is unavoidable and must be reported
    // rather than silently tolerated.
    let problem = testing::assemble(
        testing::grid(1, 1),
        testing::rooms(2),
        vec![],
        vec![testing::person("dr-who", &[])],
        vec![
            testing::with_lecturers(testing::offering("L1", 1, &[0, 1]), &[0]),
            testing::with_lecturers(testing::offering("L2", 1, &[0, 1]), &[0]),
        ],
        vec![],
        testing::all_constraints(),
    );
    let outcome = run(&problem);

    // Greedy avoids the clash by refusing to place the second session, which
    // surfaces as an unmet frequency rather than a silent double-booking.
    let kinds: Vec<&str> = outcome
        .hard_violations
        .iter()
        .map(|v| v.constraint_type)
        .collect();
    assert!(
        !kinds.is_empty(),
        "an impossible instance must report something"
    );
    assert!(
        !kinds.contains(&LECTURER_DOUBLE_BOOKING),
        "the heuristic must not resolve infeasibility by double-booking a lecturer"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — the cross-tree person clash
// ---------------------------------------------------------------------------

#[test]
fn cross_tree_person_clash_is_invisible_to_groups() {
    // Groups X and Y are separate roots. One person belongs to both.
    //
    // Step 1: with only the group check enabled, the solver sees no reason to
    // separate them and places both in the same slot. This is the failure the
    // type exists to catch — asserted here so the next step is meaningful
    // rather than vacuous.
    let group_blind = testing::cross_tree_person(testing::group_only());
    let outcome = run(&group_blind);

    assert!(
        outcome.hard_violations.is_empty(),
        "a group-only configuration reports nothing — that is the blind spot"
    );
    assert_eq!(
        slot_of(&outcome, 0),
        slot_of(&outcome, 1),
        "group-only must co-schedule them, otherwise this test proves nothing"
    );

    // Step 2: that very solution, re-evaluated with PersonDoubleBooking
    // enabled, is a violation. Same placement, different verdict.
    let person_aware = testing::cross_tree_person(testing::all_constraints());
    let violations = evaluate_hard(&person_aware, &outcome.solution);
    let person: Vec<_> = violations
        .iter()
        .filter(|v| v.constraint_type == PERSON_DOUBLE_BOOKING)
        .collect();

    assert_eq!(
        person.len(),
        1,
        "the co-scheduled solution must be a person clash, got {violations:?}"
    );
    assert!(
        person[0].detail.contains("dual-enrolled"),
        "violation should name the person, got: {}",
        person[0].detail
    );
    assert!(
        !violations
            .iter()
            .any(|v| v.constraint_type == GROUP_DOUBLE_BOOKING),
        "the groups are tree-unrelated, so the group check must NOT fire"
    );
}

#[test]
fn person_double_booking_separates_the_two_sessions() {
    // Step 3: with the person check enabled from the start, the solver avoids
    // the clash outright.
    let problem = testing::cross_tree_person(testing::all_constraints());
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(outcome.solution.placed_count(), 2);
    assert_ne!(
        slot_of(&outcome, 0),
        slot_of(&outcome, 1),
        "the dual-enrolled person must force different slots"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — a virtual Room is not an exclusive resource
// ---------------------------------------------------------------------------
//
// Online delivery is modeled AS a Room so that room-assignment logic stays
// uniform (see the proto), not to make concurrency scarce. There is one virtual
// room per delivery mode, so treating it as capacity-1 caps ALL online teaching
// at a single Session per slot, institution-wide.
//
// Both LAYERS are covered, on the same pair of instances, because they are
// separate code paths that must agree: `Occupancy` decides what the search may
// place, `check_pair` decides what gets reported. If they drift, the solver
// refuses placements it then declines to report, or the reverse — and either
// direction is caught below.
//
// Each virtual case is paired with the identical physical instance. Asserting
// only that virtual rooms permit concurrency would pass just as well against a
// build where room checking was broken outright.

#[test]
fn the_search_packs_a_virtual_room_but_not_a_physical_one() {
    // Physical: the room is exclusive, so only one of the two can be placed.
    let physical = run(&testing::two_sessions_one_room(false));
    assert_eq!(
        physical.solution.placed_count(),
        1,
        "one physical room, one slot — the second Session has nowhere to go"
    );

    // Virtual: both fit, in the same room, in the same slot.
    let virt = run(&testing::two_sessions_one_room(true));
    assert_eq!(
        virt.solution.placed_count(),
        2,
        "a virtual room hosts unlimited concurrent Sessions"
    );
    assert!(
        virt.hard_violations.is_empty(),
        "two streamed Sessions at one hour are not a clash, got {:?}",
        virt.hard_violations
    );

    let a = virt.solution.get(PlacementIdx(0)).unwrap();
    let b = virt.solution.get(PlacementIdx(1)).unwrap();
    assert_eq!((a.room, a.start), (b.room, b.start), "same room, same slot");
}

#[test]
fn a_shared_room_is_reported_only_when_the_room_is_exclusive() {
    // The search can never create a room clash, so both Sessions are pinned by
    // the caller — the "warn and allow" path that makes reporting reachable.
    let physical = testing::two_fixed_sessions_one_room(false);
    let reported = evaluate_hard(&physical, &Solution::empty(&physical));
    assert!(
        reported.iter().any(|v| v.constraint_type == ROOM_DOUBLE_BOOKING),
        "two pinned Sessions in one physical room must be reported, got {reported:?}"
    );

    let virt = testing::two_fixed_sessions_one_room(true);
    let quiet = evaluate_hard(&virt, &Solution::empty(&virt));
    assert!(
        !quiet.iter().any(|v| v.constraint_type == ROOM_DOUBLE_BOOKING),
        "the same pair in a virtual room is not a double booking, got {quiet:?}"
    );
}

// ---------------------------------------------------------------------------
// Determinism, carried forward
// ---------------------------------------------------------------------------

#[test]
fn slice2_fixtures_are_deterministic() {
    for problem in [
        testing::sibling_classes(),
        testing::deep_chain(),
        testing::lecturer_clash(),
        testing::cross_tree_person(testing::all_constraints()),
        testing::two_sessions_one_room(true),
        testing::two_sessions_one_room(false),
    ] {
        let a = run(&problem);
        let b = run(&problem);
        let pa: Vec<_> = problem.placement_ids().map(|p| a.solution.get(p)).collect();
        let pb: Vec<_> = problem.placement_ids().map(|p| b.solution.get(p)).collect();
        assert_eq!(pa, pb);
        assert_eq!(a.hard_violations, b.hard_violations);
    }
}
