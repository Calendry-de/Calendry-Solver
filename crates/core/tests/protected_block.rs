//! `ProtectedBlock`: a tenant-wide reserved window — no Session of the
//! applying `kind`s may ever be placed there, independent of any Person's or
//! Group's own data.
//!
//! Monotone-safe like the four structural types, so it is enforced as a
//! filter (`Occupancy::is_free`'s counterpart in `SearchState::is_free`),
//! never priced.

use calendry_solver_core::ids::{PlacementIdx, SlotIdx};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec, ProtectedBlockInstance};
use calendry_solver_core::search::{NeverHalt, solve};
use calendry_solver_core::testing::{self, blackout};

mod common;
use common::{SEED, moves};

fn protected(windows: Vec<calendry_solver_core::problem::Unavailability>) -> ConstraintSet {
    protected_for_kinds(windows, vec![])
}

fn protected_for_kinds(
    windows: Vec<calendry_solver_core::problem::Unavailability>,
    kinds: Vec<String>,
) -> ConstraintSet {
    ConstraintSet {
        protected_block: vec![ProtectedBlockInstance { id: "c-protected".into(), kinds, windows }],
        ..testing::structural_room_only()
    }
}

#[test]
fn a_recurring_weekly_block_is_never_chosen() {
    // Block 0 is reserved every week; 2 blocks/day, 1 week, 1 room, 1 Session.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 1, &[0])],
        constraints: protected(vec![blackout(&[], &[0], &[])]),
        ..ProblemSpec::new(testing::grid(2, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    let pl = outcome.solution.get(PlacementIdx(0)).unwrap();
    assert_ne!(pl.start, SlotIdx(0), "block 0 is reserved every week");
}

#[test]
fn a_one_off_week_only_blocks_that_week() {
    // Week 0 entirely reserved (one-off), week 1 open. 1 block/day, 2 weeks.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 1, &[0])],
        constraints: protected(vec![blackout(&[], &[], &[0])]),
        ..ProblemSpec::new(testing::grid(1, 2))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    let pl = outcome.solution.get(PlacementIdx(0)).unwrap();
    // Week 1's only slot is index 1 on this 1-block-per-day, 2-week grid.
    assert_eq!(pl.start, SlotIdx(1), "week 0 is reserved; only week 1's slot is open");
}

#[test]
fn kind_scoping_is_respected() {
    // Only ONE slot exists at all. The reservation covers "meeting" only; a
    // "lecture"-kind Offering (the testing module's default kind) must be
    // unaffected — if scoping leaked, this Offering would have nowhere left
    // to go and come up unplaced.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 1, &[0])],
        constraints: protected_for_kinds(vec![blackout(&[], &[], &[])], vec!["meeting".into()]),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0, "the reservation must not leak to an untagged kind");
}

#[test]
fn not_configured_means_no_reservation() {
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("O", 1, &[0])],
        constraints: testing::structural_room_only(),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
}
