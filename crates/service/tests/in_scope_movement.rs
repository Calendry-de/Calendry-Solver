//! `SolveScope.minimize_inscope_movement_weight` (issue #58): an in-scope
//! Session that is reused by a targeted repair carries its `original` slot
//! and room too, so the search can be charged for leaving it — the in-scope
//! counterpart of `LOCK_POLICY_MINIMIZE_MOVEMENT`'s `original`, independent
//! of `outside_scope_policy`.

use calendry_solver_core::ids::{OfferingIdx, RoomIdx, SlotIdx};

use calendry_solver::convert::convert;

mod common;
use common::{base_input, in_scope_movement_scope, offering, scope, session, slot};

#[test]
fn a_reused_in_scope_session_carries_its_original_placement() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![session("s1", "o1", slot(0, 2, 1))];

    let problem = convert(&input, &in_scope_movement_scope(&["o1"], 5.0)).expect("valid input");

    assert_eq!(problem.placements.len(), 1, "one outstanding occurrence, reusing s1's id");
    let var = &problem.placements[0];
    assert_eq!(var.offering, OfferingIdx(0));
    assert_eq!(var.existing_session_id.as_deref(), Some("s1"));
    assert_eq!(
        var.original,
        Some((slot_index(&problem, 0, 2, 1), Some(RoomIdx(0)))),
        "original must record exactly where the Session already was"
    );
    assert_eq!(problem.in_scope_movement_weight, 5.0);
    assert_eq!(problem.movement_weight, 0.0, "the two axes are independent");
}

#[test]
fn a_zero_weight_is_accepted_not_an_error() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![session("s1", "o1", slot(0, 2, 1))];

    let problem = convert(&input, &in_scope_movement_scope(&["o1"], 0.0)).expect("zero is valid");

    assert_eq!(
        problem.placements[0].original,
        Some((slot_index(&problem, 0, 2, 1), Some(RoomIdx(0))))
    );
    assert_eq!(problem.in_scope_movement_weight, 0.0);
}

#[test]
fn an_out_of_scope_reused_session_is_unaffected() {
    // o2 is not in scope at all, so it goes through `partition_sessions`'
    // out-of-scope path (hard-locked here, since `outside_scope_policy`
    // defaults to HARD), never the in-scope `reusable` map this field feeds.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0), offering("o2", 1)];
    input.existing_sessions = vec![session("s2", "o2", slot(0, 2, 1))];

    let problem = convert(&input, &in_scope_movement_scope(&["o1"], 5.0)).expect("valid input");

    assert_eq!(problem.placements.len(), 0, "o2 is out of scope under LOCK_POLICY_HARD");
    assert_eq!(problem.fixed.len(), 1);
    assert_eq!(problem.in_scope_movement_weight, 5.0, "the weight is still recorded on Problem");
}

#[test]
fn the_field_is_independent_of_outside_scope_policy() {
    // Both weights can be configured together, and each only ever governs its
    // own axis — this does not collapse into one shared knob.
    use calendry_solver_proto::v1 as pb;
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1), offering("o2", 1)];
    input.existing_sessions = vec![
        session("s1", "o1", slot(0, 2, 1)),
        session("s2", "o2", slot(0, 2, 1)),
    ];

    let s = pb::SolveScope {
        outside_scope_policy: pb::LockPolicy::MinimizeMovement as i32,
        minimize_movement_weight: 3.0,
        minimize_inscope_movement_weight: 5.0,
        ..scope(&["o1"])
    };

    let problem = convert(&input, &s).expect("valid input");

    assert_eq!(problem.movement_weight, 3.0);
    assert_eq!(problem.in_scope_movement_weight, 5.0);
    assert_eq!(problem.placements.len(), 2, "one in-scope reuse, one out-of-scope movable");
}

/// Resolve a slot the same way the grid does, so the expectation is not a
/// second hand-computed encoding of the same arithmetic.
fn slot_index(problem: &calendry_solver_core::Problem, week: u32, day: u32, block: u32) -> SlotIdx {
    problem
        .slots
        .resolve(week, day, block)
        .expect("must resolve on this grid")
}
