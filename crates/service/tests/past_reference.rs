//! ADR-0032: the answer accounts for every Session it was given, and new
//! demand never lands before the reference.
//!
//! Born as the reproducer for the "vanishing eleven" (app-side investigation,
//! 2026-09-01, runs 01a05ea6/01a05eb3): a Session before `reference_slot`
//! classified as past, satisfied its Offering's demand as fixed occupancy,
//! and came back in NEITHER `sessions` NOR `unplaced_offerings` — so the
//! app's applier deleted taught history as `not_returned_by_solver`. Each
//! re-run then invented a replacement, which nothing stopped from landing in
//! an already-elapsed week, where the NEXT run classified it past and dropped
//! it too — disjoint vanishing sets, forever.

use calendry_solver::convert::{build_output, convert};
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, offering, scope, session, slot};

const SEED: u64 = 0xC0FFEE;

fn output_with(input: &pb::SolverInput, in_scope: &[&str], budget: Budget) -> pb::SolverOutput {
    let problem = convert(input, &scope(in_scope)).expect("valid input");
    let outcome = solve(&problem, SEED, budget, &NeverHalt);
    build_output(&problem, &outcome, 0)
}

fn output_for(input: &pb::SolverInput, in_scope: &[&str]) -> pb::SolverOutput {
    output_with(input, in_scope, Budget { max_wall_millis: 0, max_moves: 20_000 })
}

#[test]
fn a_past_unlocked_session_is_retained_not_dropped() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    // Placed in week 0; the run's "now" is week 2, so it classifies Past,
    // satisfies o1's whole demand, and is nothing this run may re-place.
    input.existing_sessions = vec![session("s1", "o1", slot(0, 1, 1))];
    input.reference_slot = Some(slot(2, 1, 0));

    let output = output_for(&input, &["o1"]);

    let stats = output.stats.expect("stats are always present");
    assert_eq!(stats.termination_reason, "converged");

    // Not echoed as a placement (the caller's own data would double-count on
    // apply), not a shortfall (its Offering asked for one Session and has
    // one) — but no longer silent either: the answer names it as RETAINED,
    // so an applier reading absence as orphanhood has nothing left to
    // misread.
    assert!(output.sessions.is_empty(), "nothing was outstanding, so nothing was placed");
    assert!(output.unplaced_offerings.is_empty(), "o1 is fully realized, not short");
    assert_eq!(output.retained_session_ids, vec!["s1".to_string()]);
}

#[test]
fn new_demand_never_lands_before_the_reference() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    // "Now" is week 2 of 4 and o1 has no existing Session, so the run must
    // invent one. Weeks 0-1 are elapsed time: placing there would create a
    // Session the NEXT run classifies as past and retains — the churn engine.
    input.reference_slot = Some(slot(2, 1, 0));

    let output = output_for(&input, &["o1"]);

    assert_eq!(output.sessions.len(), 1);
    let at = output.sessions[0]
        .start_slot
        .as_ref()
        .expect("a placed session carries a slot");
    assert!(at.week >= 2, "landed in elapsed week {}", at.week);
    assert!(output.unplaced_offerings.is_empty());
}

#[test]
fn a_reference_beyond_the_term_places_nothing_and_says_so() {
    // `resolve_reference` maps a reference past the end of the term to "every
    // slot is elapsed" — the term is over. Whatever exists is retained;
    // whatever is still owed is honestly reported unplaced, with a
    // termination reason that is anything but `converged`.
    let mut input = base_input();
    input.offerings = vec![offering("kept", 1), offering("wanted", 1)];
    input.existing_sessions = vec![session("s1", "kept", slot(0, 1, 1))];
    input.reference_slot = Some(slot(99, 1, 0));

    let output = output_with(&input, &["kept", "wanted"], Budget::default());

    assert!(output.sessions.is_empty(), "no placeable slot exists");
    assert_eq!(output.retained_session_ids, vec!["s1".to_string()]);

    assert_eq!(output.unplaced_offerings.len(), 1, "only `wanted` is short");
    let short = &output.unplaced_offerings[0];
    assert_eq!(short.offering_id, "wanted");
    assert_eq!((short.requested, short.placed), (1, 0));

    let stats = output.stats.expect("stats are always present");
    assert_eq!(
        stats.termination_reason, "stagnated",
        "an unbudgeted run over unplaced demand must exhaust the ladder, never claim convergence"
    );
}

#[test]
fn locked_and_past_sessions_are_both_retained() {
    // The retained list covers every immovable Session the run received,
    // whatever made it immovable — the applier's rule is one line: a Session
    // is gone only when it appears in neither `sessions` nor here.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 3)];
    input.existing_sessions = vec![
        common::locked_session("locked-1", "o1", slot(2, 1, 1)),
        session("past-1", "o1", slot(0, 1, 1)),
    ];
    input.reference_slot = Some(slot(1, 1, 0));

    let output = output_for(&input, &["o1"]);

    let mut retained = output.retained_session_ids.clone();
    retained.sort_unstable();
    assert_eq!(retained, vec!["locked-1".to_string(), "past-1".to_string()]);
    // 3 required, 2 already realized by the immovables: exactly one new
    // placement, at or after the reference.
    assert_eq!(output.sessions.len(), 1);
    let at = output.sessions[0].start_slot.as_ref().unwrap();
    assert!(at.week >= 1, "landed in elapsed week {}", at.week);
}
