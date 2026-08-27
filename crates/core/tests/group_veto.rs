//! `GroupVeto`: a Group is away for part of the Term.
//!
//! The rule itself is `LecturerVeto` one entity across, and if that were all it
//! were, it would need barely any tests. What needs pinning is the DIRECTION the
//! blackout travels through the hierarchy, because three plausible answers exist
//! and two of them are wrong in ways no flat fixture can see:
//!
//! * `{g} ∪ ancestors(g)` — CORRECT. A Session attached to `g` is bound by `g`'s
//!   own windows and by those of everything above it, which is the same
//!   statement as "a blackout binds the Group and its descendants".
//! * `{g} ∪ descendants(g)` — lets one seminar's absence veto the lecture its
//!   entire cohort attends.
//! * `{g} ∪ ancestors ∪ descendants`, the existing conflict closure — does the
//!   same, plus more, and is the tempting one because it is already built and
//!   already used two lines away in `Problem::build`.
//!
//! With no hierarchy all three return the same set, so `two_bound_and_one_free`
//! and `a_childs_absence_does_not_bind_its_parent` are written as a pair against
//! ONE two-level fixture: the first passes under all three expansions, the
//! second only under the correct one. Deleting the second would leave a suite
//! that is fully green against a rule pointing the wrong way.
//!
//! The remaining tests cover what makes a veto a veto rather than a preference:
//! it filters during construction (not just at evaluation), it is inert when the
//! tenant has not enabled it, and its violation names the Group that actually
//! declared the window — which may be an ancestor of the one attached.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::ids::{GroupIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::ProblemSpec;
use calendry_solver_core::solution::{Occupant, Placement, SearchState};
use calendry_solver_core::testing::{self, blackout, group, group_with_blackouts};
use calendry_solver_core::{Problem, Solution};

mod common;
use common::solve_with_move_budget as run;

/// `two_day_grid` is one block on Monday and one on Saturday.
const MONDAY: SlotIdx = SlotIdx(0);
const SATURDAY: SlotIdx = SlotIdx(1);

/// Is the only Offering allowed to sit on `slot`, according to the feasibility
/// filter the search itself uses?
///
/// Deliberately `is_free` rather than a solved placement: a veto that only
/// showed up in `evaluate_hard` would let construction dead-end into a blackout
/// and then report it, instead of never going there.
fn free_at(problem: &Problem, slot: SlotIdx) -> bool {
    // `SearchState`, seeded from the immovables, is the index the search itself
    // consults — so this asks the real question rather than a reimplementation
    // of it.
    let state = SearchState::from_fixed(problem);
    let offering = &problem.offerings[0];
    let occupant = Occupant::of_offering(offering);
    let span = problem
        .slots
        .span(slot, offering.duration_blocks)
        .expect("slot in grid");

    state.is_free(problem, &occupant, &span)
}

// ---------------------------------------------------------------------------
// Direction: the pair that has to be read together
// ---------------------------------------------------------------------------

#[test]
fn a_parents_absence_binds_its_child() {
    // Cohort is away Monday; the Session is attached to its Seminar. Passes
    // under all three candidate expansions — it is here to prove the fixture
    // actually blocks something, so the mirror test below is not vacuous.
    let problem = testing::cohort_away_seminar_bound();

    assert!(!free_at(&problem, MONDAY), "the cohort's Monday must bind its seminar");
    assert!(free_at(&problem, SATURDAY), "Saturday is untouched");
}

#[test]
fn a_childs_absence_does_not_bind_its_parent() {
    // THE DISCRIMINATING HALF. Seminar is away Monday; the Session is attached
    // to the Cohort. Under `expand_subtree` or `expand_conflict` this Session
    // would be blocked on Monday, which is the bug: one seminar's block
    // placement vetoing the lecture its whole cohort attends.
    let problem = testing::seminar_away_cohort_free();

    assert!(
        free_at(&problem, MONDAY),
        "a leaf's absence must not veto its parent's Session — blackouts inherit DOWNWARD only",
    );
    assert!(free_at(&problem, SATURDAY));
}

#[test]
fn the_search_places_around_a_blackout() {
    // End to end, not just the filter: the same fixture solved must land on
    // Saturday, since Monday is the only alternative and it is blocked.
    let problem = testing::cohort_away_seminar_bound();
    let outcome = run(&problem);
    let placed = outcome.solution.get(PlacementIdx(0));

    assert_eq!(placed.map(|p| p.start), Some(SATURDAY), "must avoid the cohort's blackout");
    assert!(
        evaluate_hard(&problem, &outcome.solution).is_empty(),
        "a solution that respects the blackout reports no violation",
    );
}

// ---------------------------------------------------------------------------
// Enablement, and the inert case
// ---------------------------------------------------------------------------

#[test]
fn a_disabled_rule_is_inert_even_with_windows_on_file() {
    // The windows are Group DATA; the constraint is tenant POLICY. A tenant that
    // has not enabled `GroupVeto` must be able to record availability without it
    // steering anything — the same split `LecturerVeto` has, and the reason the
    // mask and the `Enforce` flag are separate from the lecturer ones.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![
            group_with_blackouts("cohort", None, vec![blackout(&[1], &[], &[])]),
            group("seminar", Some(0)),
        ],
        offerings: vec![testing::with_groups(testing::offering("S", 1, &[0]), &[1])],
        constraints: testing::without_group_veto(),
        ..ProblemSpec::new(testing::two_day_grid())
    });

    assert!(free_at(&problem, MONDAY), "disabled means the window does not filter");

    let outcome = run(&problem);

    assert!(
        evaluate_hard(&problem, &outcome.solution)
            .iter()
            .all(|v| v.constraint_type != ConstraintType::GroupVeto),
        "disabled means no GroupVeto violation is ever reported",
    );
}

#[test]
fn a_group_with_no_windows_changes_nothing() {
    // The counterpart of the inert case, and the reason `all_constraints()` can
    // keep this rule switched on for the whole suite: enabled with nothing
    // declared produces an empty mask, so both slots stay open.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![group("cohort", None)],
        offerings: vec![testing::with_groups(testing::offering("S", 1, &[0]), &[0])],
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::two_day_grid())
    });

    assert!(free_at(&problem, MONDAY));
    assert!(free_at(&problem, SATURDAY));
}

// ---------------------------------------------------------------------------
// What the violation says
// ---------------------------------------------------------------------------

#[test]
fn the_violation_names_the_group_that_declared_the_window() {
    // Forced into the blackout, so the evaluator has something to report. A
    // report naming "seminar" would send a timetabler to a Group with no window
    // on it; the window belongs to the cohort.
    let problem = testing::cohort_away_seminar_bound();
    let mut solution = Solution::empty(&problem);

    /*
     * `set`, not `place`. The veto is a FILTER, so the search will never put a
     * Session into a blocked slot and the evaluator is unreachable through it —
     * the same is true of `LecturerVeto`, whose suite only ever asserts the
     * negative. Recording the placement without marking occupancy is what lets
     * the reporting path be tested at all, and the report matters: it is what a
     * timetabler reads when a violation arrives in the INPUT rather than being
     * produced by a run.
     */
    solution.set(PlacementIdx(0), Some(Placement { start: MONDAY, room: RoomIdx(0) }));

    let violations = evaluate_hard(&problem, &solution);
    let veto: Vec<_> = violations
        .iter()
        .filter(|v| v.constraint_type == ConstraintType::GroupVeto)
        .collect();

    assert_eq!(veto.len(), 1, "one blocked slot, one violation: {violations:?}");
    assert!(
        veto[0].detail.contains("cohort"),
        "must name the declaring ancestor, not the attached child: {}",
        veto[0].detail,
    );
}

// ---------------------------------------------------------------------------
// The literal reading of an all-empty window
// ---------------------------------------------------------------------------

#[test]
fn all_three_axes_empty_means_always_unavailable() {
    // Preserved rather than silently treated as "never", exactly as
    // `Unavailability`'s own doc comment promises for a Person. It is a
    // representable state and the honest reading of it is total absence, so both
    // slots close and the instance is simply infeasible.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![group_with_blackouts(
            "cohort",
            None,
            vec![blackout(&[], &[], &[])],
        )],
        offerings: vec![testing::with_groups(testing::offering("S", 1, &[0]), &[0])],
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::two_day_grid())
    });

    assert!(!free_at(&problem, MONDAY));
    assert!(!free_at(&problem, SATURDAY));
}

// ---------------------------------------------------------------------------
// The closure table itself
// ---------------------------------------------------------------------------

#[test]
fn expand_ancestry_is_neither_of_the_other_two() {
    // Asserted directly on the closure, because the two expansions that would
    // be wrong here are one identifier apart from the right one at the single
    // call site that uses it.
    let problem = testing::seminar_away_cohort_free();
    let cohort = GroupIdx(0);
    let seminar = GroupIdx(1);

    assert_eq!(problem.closure.expand_ancestry(&[seminar]), vec![cohort, seminar]);
    assert_eq!(problem.closure.expand_ancestry(&[cohort]), vec![cohort], "a root has no ancestors");
    // The two it must not be, on this same fixture.
    assert_eq!(problem.closure.expand_subtree(&[cohort]), vec![cohort, seminar]);
    assert_eq!(problem.closure.expand_conflict(&[seminar]), vec![cohort, seminar]);
}
