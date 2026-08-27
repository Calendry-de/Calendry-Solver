//! Locked Sessions must count toward their Offering's required frequency.
//!
//! This is the ordinary mid-term re-solve — the primary case the lock mechanism
//! exists for. An Offering needing 12 Sessions with 3 already locked
//! (user-locked, or in the past) needs **9** more, not 12.
//!
//! Two halves of one gap made that wrong, and both are covered here:
//!
//! 1. `partition_sessions` dropped `Session.offering_id` when building a
//!    `FixedSpec`, then created placement variables for the **full**
//!    `required_session_count` — so the solver scheduled 12 on top of the 3
//!    locked ones, 15 in total.
//! 2. `FixedOccupancy` carried no Offering link either, so
//!    `constraints::exact_frequency` could not have counted the locked Sessions
//!    toward frequency even had the placements been deducted.
//!
//! These were written to be **red against the pre-fix code**, and were confirmed
//! red before the fix landed. They now live in an integration test rather than a
//! `#[cfg(test)]` module inside `convert.rs`, which is what the service crate's
//! library target made possible.

use calendry_solver::convert::convert;
use calendry_solver_core::constraints::{self, ViolationType};
use calendry_solver_core::ids::OfferingIdx;
use calendry_solver_core::search::construct;

mod common;
use common::{base_input, locked_session, offering, one_slot_grid, scope, slot};

/// Frequency violations, evaluated over a constructed solution.
///
/// This deliberately runs the solver: "how many Sessions realize this Offering"
/// is only observable once something has been placed. It is the one assertion
/// here that reaches past `convert`'s own interface, and it does so because the
/// fact under test — the placement/lock split — is what `Problem` exposes
/// through `residual_for` and what `exact_frequency` consumes.
fn frequency_violations(problem: &calendry_solver_core::Problem) -> Vec<constraints::Violation> {
    let (solution, _) = construct(problem);
    constraints::evaluate_hard(problem, &solution)
        .into_iter()
        .filter(|v| v.constraint_type == ViolationType::ExactFrequency)
        .collect()
}

#[test]
fn locked_sessions_count_toward_required_frequency() {
    // 12 required, 3 already locked. The run must place exactly 9 more.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 12)];
    input.existing_sessions = vec![
        locked_session("s1", "o1", slot(0, 1, 1)),
        locked_session("s2", "o1", slot(0, 2, 1)),
        locked_session("s3", "o1", slot(0, 3, 1)),
    ];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    assert_eq!(
        problem.fixed.len(),
        3,
        "the three locked Sessions must survive as immovable occupancy"
    );

    // RED BEFORE THE FIX: this was 12, so the solver scheduled 12 on top of the
    // 3 locked ones — 15 Sessions for an Offering requiring 12.
    assert_eq!(
        problem.placements.len(),
        9,
        "12 required minus 3 locked = 9 placements to position"
    );

    // RED BEFORE THE FIX: the link did not exist, so frequency had nothing to
    // count.
    assert_eq!(
        problem.immovable_count(OfferingIdx(0)),
        3,
        "locked Sessions must carry their Offering link"
    );
    assert_eq!(
        problem.residual_for(OfferingIdx(0)),
        0,
        "9 placements plus 3 locks exactly accounts for the 12 required"
    );

    // RED BEFORE THE FIX: 12 required against 9 placed reported a violation that
    // does not exist.
    let freq = frequency_violations(&problem);
    assert!(
        freq.is_empty(),
        "a fully realized Offering must report no frequency violation, got {freq:#?}"
    );
}

#[test]
fn a_genuine_shortfall_is_still_reported() {
    // The counterpart: counting locked Sessions must not become a way to silence
    // a real shortfall. One slot, two rooms, so 5 cannot be reached.
    let mut input = base_input();
    one_slot_grid(&mut input);
    input.offerings = vec![offering("o1", 5)];
    input.existing_sessions = vec![locked_session("s1", "o1", slot(0, 1, 0))];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.placements.len(), 4, "5 required minus 1 locked");

    assert!(
        !frequency_violations(&problem).is_empty(),
        "a genuine shortfall must still be reported"
    );
}

#[test]
fn out_of_scope_offerings_are_unaffected() {
    // o2 is not in scope, so it gets no placement variables and its frequency is
    // not this run's business. Its locked Sessions must still occupy the grid
    // without leaking a violation or perturbing o1.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 4), offering("o2", 7)];
    input.existing_sessions = vec![
        locked_session("s1", "o1", slot(0, 1, 1)),
        locked_session("s2", "o2", slot(0, 2, 1)),
        locked_session("s3", "o2", slot(0, 3, 1)),
    ];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    assert_eq!(problem.placements.len(), 3, "only o1 is in scope: 4 required minus its 1 lock");
    assert_eq!(problem.fixed.len(), 3, "all three locks remain occupancy");
    assert!(problem.in_scope(OfferingIdx(0)));
    assert!(!problem.in_scope(OfferingIdx(1)));

    let freq = frequency_violations(&problem);
    assert!(
        freq.is_empty(),
        "an out-of-scope Offering must not report a frequency violation, got {freq:#?}"
    );
}

/// The over-supply fixture: 2 required, 4 locked.
fn over_supplied_input() -> calendry_solver_proto::v1::SolverInput {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 2)];
    input.existing_sessions = vec![
        locked_session("s1", "o1", slot(0, 1, 1)),
        locked_session("s2", "o1", slot(0, 2, 1)),
        locked_session("s3", "o1", slot(0, 3, 1)),
        locked_session("s4", "o1", slot(0, 4, 1)),
    ];
    input
}

#[test]
fn more_locks_than_required_saturates_instead_of_underflowing() {
    // The app's editing UX is "warn and allow", so a caller can legitimately
    // send more Sessions than the Offering claims to need. The deduction must
    // saturate at zero rather than wrapping a u32 into four billion placement
    // variables.
    let problem = convert(&over_supplied_input(), &scope(&["o1"])).expect("valid input");
    assert_eq!(
        problem.placements.len(),
        0,
        "already over-supplied: nothing left to place, and no underflow"
    );
}

#[test]
fn an_over_supplied_offering_reports_a_frequency_violation() {
    // RED against the previous behaviour, which reported nothing here — a known
    // gap that was itself asserted, so that changing it had to be deliberate.
    //
    // The cause was that `exact_frequency` used "owns at least one placement
    // variable" as its proxy for "in scope". Deducting locked Sessions is the one
    // thing that can drive an in-scope Offering's placement count to zero, so an
    // over-supplied Offering looked exactly like an out-of-scope one and was
    // skipped. `Problem` now carries real scope membership, resolved at this
    // boundary and passed through `ProblemSpec`.
    let problem = convert(&over_supplied_input(), &scope(&["o1"])).expect("valid input");

    assert!(
        problem.in_scope(OfferingIdx(0)),
        "zero placements must not be mistaken for out of scope"
    );
    assert_eq!(
        problem.residual_for(OfferingIdx(0)),
        -2,
        "2 required against 4 locked is a surplus of two"
    );

    let freq = frequency_violations(&problem);
    assert_eq!(freq.len(), 1, "over-supply must be reported, got {freq:#?}");
    assert!(
        freq[0].detail.contains("requires 2 session(s), 4 placed"),
        "the violation must state both counts, got {:?}",
        freq[0].detail
    );
}

#[test]
fn an_out_of_scope_over_supplied_offering_still_reports_nothing() {
    // The reason real scope membership was needed rather than simply dropping
    // the in-scope gate: an Offering nobody asked about is not this run's
    // business however many Sessions it has.
    let mut input = over_supplied_input();
    input.offerings.push(offering("o2", 1));

    let problem = convert(&input, &scope(&["o2"])).expect("valid input");
    assert!(!problem.in_scope(OfferingIdx(0)));
    assert!(frequency_violations(&problem).is_empty(), "o1 is out of scope; o2 is satisfiable");
}
