//! The spare bank (issue #22): a Session that is OWED but currently unplaced,
//! after a cancellation. It reaches the solver as an ordinary entry in
//! `existing_sessions` with no `start_slot`.
//!
//! Nothing new represents it. `PlacementVar` already separates *which Session
//! id this occurrence realizes* (`existing_session_id`) from *where it already
//! sat* (`original`, an `Option` whose `None` already means "nothing to be
//! charged for leaving a place it never held"). A banked Session is exactly
//! `Some(id)` with a `None` original — so it claims one of its Offering's
//! outstanding occurrences, keeps its identity, and is placed free of any
//! movement charge, because there is nowhere it is being moved from.
//!
//! The refusals live in `rejections.rs`; this file is the accepting half.

use calendry_solver_core::ids::{OfferingIdx, RoomIdx};

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, in_scope_movement_scope, offering, scope, session, slot};

/// A Session that exists and is owed, but sits nowhere.
fn banked(id: &str, offering_id: &str) -> pb::Session {
    pb::Session {
        start_slot: None,
        room_id: String::new(),
        ..session(id, offering_id, slot(0, 1, 0))
    }
}

#[test]
fn a_banked_session_keeps_its_id_and_carries_no_original() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![banked("s1", "o1")];

    let problem = convert(&input, &scope(&["o1"])).expect("an unplaced Session is legitimate");

    assert_eq!(problem.placements.len(), 1, "the Offering still owes its one Session");
    let var = &problem.placements[0];
    assert_eq!(var.offering, OfferingIdx(0));
    assert_eq!(
        var.existing_session_id.as_deref(),
        Some("s1"),
        "the banked Session's identity must survive — the point of a bank is that \
         what is owed is not silently forgotten"
    );
    assert_eq!(var.original, None, "there is nowhere it is being moved from");
}

#[test]
fn a_banked_session_does_not_add_demand_on_top_of_the_required_count() {
    // It IS one of the Offering's required Sessions, not an extra one. An
    // Offering needing 3, with one banked, still needs exactly 3 placed.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 3)];
    input.existing_sessions = vec![banked("s1", "o1")];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.placements.len(), 3);
    let reused: Vec<_> = problem
        .placements
        .iter()
        .filter_map(|p| p.existing_session_id.as_deref())
        .collect();
    assert_eq!(reused, vec!["s1"], "exactly one occurrence reuses the banked id");
}

#[test]
fn a_banked_session_is_never_charged_for_movement() {
    // With in-scope movement pressure configured, a REUSED placed Session is
    // charged for leaving its slot. A banked one has no slot to leave, so
    // every placement of it must be free — otherwise the search would be
    // biased away from rescheduling exactly the Session the bank exists to
    // get rescheduled.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![banked("s1", "o1")];

    let problem = convert(&input, &in_scope_movement_scope(&["o1"], 500.0)).expect("valid input");
    assert_eq!(problem.in_scope_movement_weight, 500.0, "the weight is configured");

    let p = calendry_solver_core::ids::PlacementIdx(0);
    for slot_index in 0..8u32 {
        let at = calendry_solver_core::ids::SlotIdx(slot_index);
        assert_eq!(
            problem.movement_cost(p, at, RoomIdx(0)),
            0.0,
            "a banked Session must be free to place anywhere"
        );
    }
}

#[test]
fn a_placed_session_outranks_a_banked_one_when_occurrences_are_scarce() {
    // The tiebreak that is load-bearing rather than cosmetic. One outstanding
    // occurrence, two candidates for it: a placed Session and a banked one.
    // The placed one must win — it would forfeit not just its id but its
    // `original`, which seeds construction back at its existing slot and
    // spares it the movement charge. The banked one has no `original` to
    // forfeit.
    //
    // "s0" sorts BEFORE "s1", so an id-only sort would hand the occurrence to
    // the banked Session and silently churn a Session nobody asked to move.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![banked("s0", "o1"), session("s1", "o1", slot(0, 2, 1))];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    assert_eq!(problem.placements.len(), 1, "one outstanding occurrence");
    let var = &problem.placements[0];
    assert_eq!(var.existing_session_id.as_deref(), Some("s1"), "the PLACED Session wins");
    assert!(var.original.is_some(), "and keeps its original, so it is not gratuitously moved");
}

#[test]
fn banked_sessions_reuse_ids_deterministically() {
    // Two banked Sessions and two outstanding occurrences: the mapping must
    // not depend on the caller's ordering of `existing_sessions`.
    let assign = |order: Vec<pb::Session>| {
        let mut input = base_input();
        input.offerings = vec![offering("o1", 2)];
        input.existing_sessions = order;
        let problem = convert(&input, &scope(&["o1"])).expect("valid input");
        problem
            .placements
            .iter()
            .map(|p| p.existing_session_id.clone())
            .collect::<Vec<_>>()
    };

    let forward = assign(vec![banked("s1", "o1"), banked("s2", "o1")]);
    let reversed = assign(vec![banked("s2", "o1"), banked("s1", "o1")]);
    assert_eq!(forward, reversed);
    assert_eq!(forward, vec![Some("s1".to_string()), Some("s2".to_string())]);
}

#[test]
fn an_out_of_scope_banked_session_is_ignored_rather_than_refused() {
    // A repair scoped to one Offering must not fail because some OTHER
    // Offering happens to have a banked Session. It is not occupancy either
    // — it sits nowhere — so it is genuinely nothing to this run.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1), offering("o2", 1)];
    input.existing_sessions = vec![banked("s2", "o2")];

    let problem = convert(&input, &scope(&["o1"])).expect("must not refuse the whole run");
    assert_eq!(problem.placements.len(), 1, "only o1 is being placed");
    assert_eq!(problem.placements[0].offering, OfferingIdx(0));
    assert!(problem.fixed.is_empty(), "an unplaced Session is not occupancy");
}

#[test]
fn a_banked_session_naming_an_unknown_offering_is_ignored() {
    // The same "warn and allow" tolerance a PLACED Session naming a missing
    // Offering gets, minus the occupancy that made a placed one still matter.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![banked("s9", "gone")];

    let problem = convert(&input, &scope(&["o1"])).expect("must not refuse the whole run");
    assert_eq!(problem.placements.len(), 1);
    assert_eq!(problem.placements[0].existing_session_id, None);
}

#[test]
fn a_banked_session_still_counts_as_owed_when_locks_already_realize_some() {
    // `ExactFrequency` accounting is unchanged: an immovable Session deducts
    // from the outstanding count, and the bank claims one of what remains.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 2)];
    input.existing_sessions = vec![
        pb::Session { is_locked: true, ..session("s-locked", "o1", slot(0, 2, 1)) },
        banked("s-banked", "o1"),
    ];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.fixed.len(), 1, "the locked Session is immovable occupancy");
    assert_eq!(problem.placements.len(), 1, "2 required minus 1 already realized");
    assert_eq!(problem.placements[0].existing_session_id.as_deref(), Some("s-banked"));
    assert_eq!(problem.placements[0].original, None);
}
