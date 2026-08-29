//! `LOCK_POLICY_MINIMIZE_MOVEMENT`: an out-of-scope Session becomes a movable
//! placement instead of hard-locked occupancy, carrying its `original` slot
//! and room so the search can be charged for leaving it.
//!
//! ADR-0008 relaxes exactly one `Immovable` variant — `OutOfScope` — and only
//! this policy relaxes it. `Locked`, `Past` and `External` are untouched in
//! every version, which is what most of these tests actually check: the
//! interesting failure mode is not "does the happy path work" but "does this
//! policy accidentally relax something ADR-0008 says it must not".

use calendry_solver_core::ids::{OfferingIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::Immovable;

use calendry_solver::convert::convert;

mod common;
use common::{base_input, locked_session, minimize_movement_scope, offering, scope, session, slot};

#[test]
fn an_out_of_scope_session_becomes_movable_carrying_its_original_placement() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0), offering("o2", 1)];
    // o2 is not in the request's scope, and this Session is neither locked nor
    // past — under LOCK_POLICY_HARD it would be pure occupancy.
    input.existing_sessions = vec![session("s2", "o2", slot(0, 2, 1))];

    let problem = convert(&input, &minimize_movement_scope(&["o1"], 5.0))
        .expect("a valid out-of-scope Session under a valid weight");

    assert_eq!(problem.fixed.len(), 0, "the Session must not survive as hard occupancy");
    assert_eq!(problem.placements.len(), 1, "it must become exactly one placement variable");

    let var = &problem.placements[0];
    assert_eq!(var.offering, OfferingIdx(1), "attached to o2, which owns the Session");
    assert_eq!(var.existing_session_id.as_deref(), Some("s2"));
    assert_eq!(
        var.original,
        Some((slot_index(&problem, 0, 2, 1), Some(RoomIdx(0)))),
        "original must record exactly where the Session already was"
    );
    assert_eq!(problem.movement_weight, 5.0);
}

#[test]
fn the_same_input_stays_hard_locked_under_lock_policy_hard() {
    // The contrast that proves the split is policy-driven, not automatic: same
    // out-of-scope Session, only the policy differs.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0), offering("o2", 1)];
    input.existing_sessions = vec![session("s2", "o2", slot(0, 2, 1))];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    assert_eq!(problem.fixed.len(), 1, "v1 hard-locks everything outside scope");
    assert_eq!(problem.placements.len(), 0, "o2 is out of scope: no placement variables at all");
    assert_eq!(problem.movement_weight, 0.0, "LOCK_POLICY_HARD carries no movement weight");
}

#[test]
fn a_locked_out_of_scope_session_stays_hard_locked_under_minimize_movement() {
    // `Locked` outranks `OutOfScope` in `classify_immovable`, and that
    // precedence must survive the new policy: v2 relaxes ONLY `OutOfScope`,
    // never `Locked`, however the caller configures the policy.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0), offering("o2", 1)];
    input.existing_sessions = vec![locked_session("s2", "o2", slot(0, 2, 1))];

    let problem = convert(&input, &minimize_movement_scope(&["o1"], 5.0)).expect("valid input");

    assert_eq!(problem.fixed.len(), 1, "an explicit lock must never become movable");
    assert_eq!(problem.placements.len(), 0);
    assert_eq!(problem.fixed[0].reason, Immovable::Locked);
}

#[test]
fn a_past_out_of_scope_session_stays_hard_locked_under_minimize_movement() {
    // `Past` is a correctness rule, not a preference, in both versions.
    let mut input = base_input();
    input.reference_slot = Some(slot(0, 3, 0));
    input.offerings = vec![offering("o1", 0), offering("o2", 1)];
    // Week 0 day 2 is before the week 0 day 3 reference.
    input.existing_sessions = vec![session("s2", "o2", slot(0, 2, 1))];

    let problem = convert(&input, &minimize_movement_scope(&["o1"], 5.0)).expect("valid input");

    assert_eq!(problem.fixed.len(), 1, "a past Session must never become movable");
    assert_eq!(problem.placements.len(), 0);
    assert_eq!(problem.fixed[0].reason, Immovable::Past);
}

#[test]
fn an_ad_hoc_out_of_scope_session_is_not_relaxed_by_minimize_movement() {
    // A `PlacementVar` has no room for its own occupant data — every other
    // placement is governed entirely by its Offering — so a Session realizing
    // no Offering at all has nothing for "movable" to mean. It must stay hard
    // occupancy regardless of policy.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.existing_sessions = vec![session("staff-meeting", "", slot(0, 2, 1))];

    let problem = convert(&input, &minimize_movement_scope(&["o1"], 5.0)).expect("valid input");

    assert_eq!(problem.fixed.len(), 1, "an ad-hoc Session has no Offering to attach movable to");
    assert_eq!(problem.placements.len(), 0);
    assert_eq!(problem.fixed[0].reason, Immovable::OutOfScope);
}

#[test]
fn a_zero_movement_weight_is_accepted_not_an_error() {
    // The same reading every other soft weight gives a zero: "track whether it
    // moved, but do not steer against it" is a legitimate configuration, not a
    // contradiction of "minimize movement".
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0), offering("o2", 1)];
    input.existing_sessions = vec![session("s2", "o2", slot(0, 2, 1))];

    let problem = convert(&input, &minimize_movement_scope(&["o1"], 0.0)).expect("zero is valid");

    assert_eq!(problem.placements.len(), 1, "still movable — the weight is zero, not the policy");
    assert_eq!(problem.movement_weight, 0.0);
}

/// Resolve a slot the same way the grid does, so the expectation is not a
/// second hand-computed encoding of the same arithmetic.
fn slot_index(problem: &calendry_solver_core::Problem, week: u32, day: u32, block: u32) -> SlotIdx {
    problem
        .slots
        .resolve(week, day, block)
        .expect("must resolve on this grid")
}
