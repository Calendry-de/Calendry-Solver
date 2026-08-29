//! `GroupSizeFitsRoom`: cross-checks a placement's Room capacity against its
//! Offering's own Groups' summed `Group.size`, independent of whatever
//! `Offering.min_capacity` claims.
//!
//! HARD, validation-shaped — a safety net against bad input data, not a
//! preference the search steers toward. Evaluated over placed Sessions only,
//! same convention as `LecturerVeto`/`GroupVeto`.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::problem::{ConstraintInstance, ConstraintSet, ProblemSpec};
use calendry_solver_core::testing::{self, group_with_size};

mod common;
use common::solve_with_move_budget as run;

fn enabled() -> ConstraintSet {
    ConstraintSet {
        group_size_fits_room: vec![ConstraintInstance { id: "c-size".into(), kinds: vec![] }],
        ..testing::structural_room_only()
    }
}

#[test]
fn a_room_too_small_for_the_offerings_own_group_is_reported() {
    let spec = ProblemSpec {
        rooms: vec![testing::room("R0")], // capacity 30 by default
        groups: vec![group_with_size("G", None, 40)],
        offerings: vec![testing::with_groups(testing::offering("O", 1, &[0]), &[0])],
        constraints: enabled(),
        ..ProblemSpec::new(testing::grid(1, 1))
    };
    let problem = testing::assemble(spec);
    let outcome = run(&problem);

    let violations = evaluate_hard(&problem, &outcome.solution);
    assert!(
        violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::GroupSizeFitsRoom),
        "40 attendees in a 30-seat room must be reported: {violations:?}"
    );
}

#[test]
fn a_room_that_fits_is_not_reported() {
    let spec = ProblemSpec {
        rooms: vec![testing::room("R0")],
        groups: vec![group_with_size("G", None, 20)],
        offerings: vec![testing::with_groups(testing::offering("O", 1, &[0]), &[0])],
        constraints: enabled(),
        ..ProblemSpec::new(testing::grid(1, 1))
    };
    let problem = testing::assemble(spec);
    let outcome = run(&problem);

    let violations = evaluate_hard(&problem, &outcome.solution);
    assert!(
        !violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::GroupSizeFitsRoom),
        "20 attendees fit a 30-seat room: {violations:?}"
    );
}

#[test]
fn disabled_leaves_an_oversized_group_unreported() {
    let spec = ProblemSpec {
        rooms: vec![testing::room("R0")],
        groups: vec![group_with_size("G", None, 40)],
        offerings: vec![testing::with_groups(testing::offering("O", 1, &[0]), &[0])],
        // No `group_size_fits_room` instance at all.
        constraints: testing::structural_room_only(),
        ..ProblemSpec::new(testing::grid(1, 1))
    };
    let problem = testing::assemble(spec);
    let outcome = run(&problem);

    let violations = evaluate_hard(&problem, &outcome.solution);
    assert!(
        !violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::GroupSizeFitsRoom),
        "the tenant never enabled this check: {violations:?}"
    );
}

#[test]
fn a_multi_room_placements_capacity_is_the_sum_across_its_rooms() {
    // 2 Rooms of 20 each, required together — matches the multi-Room
    // "capacity is summed" convention. 35 attendees fit 40 summed seats even
    // though neither Room alone would hold them.
    let offering = testing::with_room_combinations(
        testing::with_groups(testing::offering("O", 1, &[]), &[0]),
        2,
        &[0, 1],
    );
    let spec = ProblemSpec {
        rooms: testing::rooms(2), // capacity 30 each by default -> testing::room's capacity
        groups: vec![group_with_size("G", None, 35)],
        offerings: vec![offering],
        constraints: enabled(),
        ..ProblemSpec::new(testing::grid(1, 1))
    };
    let problem = testing::assemble(spec);
    let outcome = run(&problem);

    let violations = evaluate_hard(&problem, &outcome.solution);
    assert!(
        !violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::GroupSizeFitsRoom),
        "35 attendees fit 2 Rooms of 30 each, summed: {violations:?}"
    );
}
