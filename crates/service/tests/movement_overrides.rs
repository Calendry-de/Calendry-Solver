//! `SolveScope.movement_overrides` (issue #70): per-Person and per-Group
//! exceptions to the two run-wide movement weights, so a repair can mark some
//! people and cohorts as fine to move and others as soft-unmovable.
//!
//! Still SOFT throughout, and distinct from a Session `lock`, which is a HARD
//! exemption — an override can never stop a Session from moving, only make it
//! expensive. The resolution itself (lecturers only, Group entries binding
//! downward, largest-wins) is unit-tested in `crates/core`'s `problem`
//! module; this file covers the wire boundary and its refusals.

use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{
    base_input, group_movement_override, offering, person_movement_override, scope, session, slot,
};

/// One in-scope Offering with one existing Session, in-scope movement
/// pressure at `weight`, plus whatever overrides the test wants. The Session
/// therefore carries an `original` and there is a real move to charge.
fn repair_scope(weight: f64, overrides: Vec<pb::MovementOverride>) -> pb::SolveScope {
    pb::SolveScope {
        minimize_inscope_movement_weight: weight,
        movement_overrides: overrides,
        ..scope(&["o1"])
    }
}

fn repair_input() -> pb::SolverInput {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![session("s1", "o1", slot(0, 2, 1))];
    input
}

/// What moving the reused Session away from its `original` costs.
fn move_cost(scope: &pb::SolveScope) -> f64 {
    let problem = convert(&repair_input(), scope).expect("valid input");
    let original = problem.placements[0]
        .original
        .expect("the reused Session must carry an original");
    let elsewhere = SlotIdx(original.0.get() as u32 + 1);
    problem.movement_cost(PlacementIdx(0), elsewhere, RoomIdx(0))
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

#[test]
fn no_overrides_leaves_the_scope_wide_weight_in_charge() {
    assert_eq!(move_cost(&repair_scope(5.0, vec![])), 5.0);
}

#[test]
fn a_person_override_resolves_and_replaces_the_scope_wide_weight() {
    // p1 is `offering`'s only candidate lecturer. Asserted through the cost
    // rather than through a resolved list: `Problem` folds the overrides into
    // one per-Offering weight at build time and keeps no list to inspect,
    // which is exactly what keeps `movement_cost` a single indexed read.
    let s = repair_scope(5.0, vec![person_movement_override("p1", 50.0)]);
    assert_eq!(move_cost(&s), 50.0);
}

#[test]
fn a_group_override_resolves_and_replaces_the_scope_wide_weight() {
    // g1 is `offering`'s only Group.
    let s = repair_scope(5.0, vec![group_movement_override("g1", 50.0)]);
    assert_eq!(move_cost(&s), 50.0);
}

#[test]
fn a_zero_weight_override_is_a_real_setting_not_an_unset_field() {
    // "Movable, no extra cost" is half of what the issue asks for, and it has
    // to survive a large scope-wide weight — so an override REPLACES the base
    // rather than maxing against it.
    let s = repair_scope(500.0, vec![person_movement_override("p1", 0.0)]);
    assert_eq!(move_cost(&s), 0.0);
}

#[test]
fn the_two_run_wide_weights_are_still_recorded_alongside_the_overrides() {
    // Overrides are an exception mechanism, not a replacement: an Offering no
    // override covers still reads the run-wide weight, so both must survive
    // conversion.
    let s = repair_scope(5.0, vec![person_movement_override("p1", 50.0)]);
    let problem = convert(&repair_input(), &s).expect("valid input");
    assert_eq!(problem.in_scope_movement_weight, 5.0);
    assert_eq!(problem.movement_weight, 0.0);
}

// ---------------------------------------------------------------------------
// Refusals. Every one of these is an error rather than a skip for the same
// reason `build_relations` refuses a dangling member: an override the caller
// sent and the solver silently dropped would let a run be reported as
// respecting a protection it never applied.
// ---------------------------------------------------------------------------

#[test]
fn an_unresolvable_person_id_is_refused() {
    let s = repair_scope(5.0, vec![person_movement_override("nobody", 50.0)]);
    let e = convert(&repair_input(), &s).expect_err("a dangling person must be refused");
    assert!(
        matches!(e, ConvertError::UnknownPerson { ref person, .. } if person == "nobody"),
        "got {e:?}"
    );
}

#[test]
fn an_unresolvable_group_id_is_refused() {
    let s = repair_scope(5.0, vec![group_movement_override("nowhere", 50.0)]);
    let e = convert(&repair_input(), &s).expect_err("a dangling group must be refused");
    assert!(
        matches!(e, ConvertError::UnknownGroup { ref group, .. } if group == "nowhere"),
        "got {e:?}"
    );
}

#[test]
fn an_override_with_no_target_is_refused() {
    let s = repair_scope(5.0, vec![pb::MovementOverride { target: None, weight: 50.0 }]);
    let e = convert(&repair_input(), &s).expect_err("a targetless override applies to nothing");
    assert!(matches!(e, ConvertError::MovementOverrideWithoutTarget { index: 0 }), "got {e:?}");
}

#[test]
fn a_negative_weight_is_refused() {
    // Same reason every other movement weight must be >= 0: the term declares
    // minimize, so a negative weight would REWARD moving the very Sessions
    // the override was sent to protect.
    let s = repair_scope(5.0, vec![person_movement_override("p1", -1.0)]);
    let e = convert(&repair_input(), &s).expect_err("a negative weight inverts the term");
    assert!(
        matches!(e, ConvertError::NegativeMovementOverrideWeight { index: 0, weight } if weight == -1.0),
        "got {e:?}"
    );
}

#[test]
fn the_refused_index_names_which_override_was_wrong() {
    // With a list rather than a scalar, "one of your overrides is invalid" is
    // not actionable — the message has to say which.
    let s = repair_scope(
        5.0,
        vec![
            person_movement_override("p1", 1.0),
            group_movement_override("g1", 2.0),
            person_movement_override("nobody", 3.0),
        ],
    );
    let e = convert(&repair_input(), &s).expect_err("the third is dangling");
    let message = e.to_string();
    assert!(message.contains("movement_overrides[2]"), "got {message}");
}
