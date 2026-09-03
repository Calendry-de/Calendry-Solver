//! `LecturerVeto` over a genuine lecturer POOL (Calendry #131).
//!
//! A pool Offering's lecturers are a search-time choice, so its
//! `Offering::veto_slots` — the per-Offering mask precomputed from
//! `Offering::lecturers` — is unconditionally EMPTY. Until this change that
//! emptiness was why `LecturerVeto` plus a pool had to be refused at
//! conversion: the mask passed every fixed-assignment test and blocked nothing
//! for a pool. The fix is ADR-0034's, one axis over: a per-Person mask built
//! once in `Problem::build`, asked against the candidate's CHOSEN lecturers
//! through `Problem::lecturer_veto_blocks`.
//!
//! The guard is the mirrored pair ADR-0027 and ADR-0034 both use.
//! `a_veto_binds_a_fixed_assignment` passes under the old per-Offering mask
//! AND under the live check, and exists only so its mirror is not vacuously
//! green; `a_veto_binds_a_pool_offering` passes under the live check alone.
//!
//! Assertions are mostly `is_free`, because this is a filter: the search never
//! produces a placement in a blackout, so asking the filter directly is the
//! only way to see it decide.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::ids::{PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::ProblemSpec;
use calendry_solver_core::solution::{MAX_LECTURERS, Occupant, Placement, SearchState};
use calendry_solver_core::{Problem, Solution, testing};

mod common;
use common::solve_with_move_budget as run;

const P0: u32 = 0;
const P1: u32 = 1;

fn chosen(lecturers: &[u32]) -> [Option<PersonIdx>; MAX_LECTURERS] {
    let mut out = [None; MAX_LECTURERS];
    for (slot, &l) in out.iter_mut().zip(lecturers) {
        *slot = Some(PersonIdx(l));
    }
    out
}

/// May the only Offering start at `block` in the only Room, led by
/// `lecturers` chosen from its pool (empty for a fixed assignment), according
/// to the filter the search itself consults?
fn free_at(problem: &Problem, block: u32, lecturers: &[u32]) -> bool {
    let state = SearchState::from_fixed(problem);
    let offering = &problem.offerings[0];
    let span = problem
        .slots
        .span(SlotIdx(block), offering.duration_blocks)
        .expect("slot in grid");
    let mut occupant = Occupant::of_offering(offering).with_room(RoomIdx(0));
    if !lecturers.is_empty() {
        occupant = occupant.with_pool_lecturers(chosen(lecturers));
    }
    state.is_free(problem, &occupant, &span)
}

/// Two blocks, one Room, one Session, led by P0 alone, who is away at block 0.
fn fixed_assignment() -> Problem {
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person_with_blackouts(
            "P0",
            &[],
            vec![testing::blackout(&[], &[0], &[])],
        )],
        offerings: vec![testing::with_lecturers(
            testing::offering("S", 1, &[0]),
            &[P0],
        )],
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::grid(2, 1))
    })
}

/// Two blocks, one Room, `sessions` Sessions, led by ONE of {P0, P1}: P0 is
/// away at block 0 and P1 at block 1, so every block has exactly one
/// available candidate and they are different people.
fn pool(sessions: u32, constraints: calendry_solver_core::problem::ConstraintSet) -> Problem {
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![
            testing::person_with_blackouts("P0", &[], vec![testing::blackout(&[], &[0], &[])]),
            testing::person_with_blackouts("P1", &[], vec![testing::blackout(&[], &[1], &[])]),
        ],
        offerings: vec![testing::with_lecturer_pool(
            testing::offering("S", sessions, &[0]),
            1,
            &[P0, P1],
        )],
        constraints,
        ..ProblemSpec::new(testing::grid(2, 1))
    })
}

// ---------------------------------------------------------------------------
// The guard pair — one rule, two mirrored assertions (§ADR-0034)
// ---------------------------------------------------------------------------

#[test]
fn a_veto_binds_a_fixed_assignment() {
    // Passes under the live check AND under the per-Offering mask
    // precomputed from `Offering::lecturers`, which is populated here. Present
    // only so its mirror below is not vacuously green.
    let problem = fixed_assignment();

    assert!(!free_at(&problem, 0, &[]), "P0 is away at block 0");
    assert!(free_at(&problem, 1, &[]), "and present at block 1");
}

#[test]
fn a_veto_binds_a_pool_offering() {
    // THE DISCRIMINATING TEST. The pool Offering's `Offering::lecturers` is
    // EMPTY, so the precomputed mask is empty too and would permit every
    // block for everyone. Only a check against the CHOSEN lecturer gets this
    // right — and the answer must depend on WHO was chosen, which is the
    // whole claim.
    let problem = pool(1, testing::all_constraints());

    assert!(
        problem.offerings[0].veto_slots.iter().next().is_none(),
        "the per-Offering mask is empty for a pool; it is NOT where the veto lives"
    );

    assert!(!free_at(&problem, 0, &[P0]), "P0 is away at block 0");
    assert!(free_at(&problem, 1, &[P0]), "P0 is present at block 1");
    assert!(free_at(&problem, 0, &[P1]), "P1 is present at block 0");
    assert!(!free_at(&problem, 1, &[P1]), "P1 is away at block 1");
}

// ---------------------------------------------------------------------------
// The search chooses, honouring each person's own calendar
// ---------------------------------------------------------------------------

#[test]
fn the_search_picks_whichever_candidate_is_available_at_each_block() {
    // The case the ticket opens with: two named people, "the solver picks
    // one". Two Sessions in one Room must take both blocks, and each block
    // has exactly one available candidate — so the ONLY feasible timetable
    // gives block 0 to P1 and block 1 to P0. Nothing in the input says so
    // directly; the search has to choose the lecturer per Session against
    // each person's own blackouts, which is what the refusal used to forbid.
    let problem = pool(2, testing::all_constraints());
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);

    let mut by_block: Vec<(u32, Vec<u32>)> = problem
        .placement_ids()
        .map(|p| {
            let pl = outcome
                .solution
                .get(p)
                .expect("both Sessions are placeable");
            let leads: Vec<u32> = pl.lecturers.iter().flatten().map(|l| l.0).collect();
            (pl.start.0, leads)
        })
        .collect();
    by_block.sort();

    assert_eq!(by_block, vec![(0, vec![P1]), (1, vec![P0])]);
}

#[test]
fn without_the_rule_the_pool_veto_is_inert() {
    // Falsification: the blackout VALUES are still on the Persons, and with
    // the switch off they block nothing. So the tests above measure the rule,
    // not the fixture.
    let problem = pool(1, testing::without_lecturer_veto());

    assert!(free_at(&problem, 0, &[P0]));
    assert!(free_at(&problem, 1, &[P1]));
}

// ---------------------------------------------------------------------------
// The authoritative report reads the SAME chosen lecturers (§ADR-0014)
// ---------------------------------------------------------------------------

#[test]
fn a_pool_placement_in_its_chosen_lecturers_blackout_is_reported_naming_them() {
    // The search cannot produce this Solution — the filter refuses it — so it
    // is built by hand. Reading `Offering::veto_slots` here would report
    // nothing: the mask is empty for a pool. The report has to read the
    // Placement's chosen lecturers, the way the filter does.
    let problem = pool(1, testing::all_constraints());

    let report = |lecturer: u32| {
        let mut solution = Solution::empty(&problem);
        let mut placement = Placement::single(SlotIdx(0), RoomIdx(0));
        placement.lecturers = chosen(&[lecturer]);
        solution.set(PlacementIdx(0), Some(placement));
        evaluate_hard(&problem, &solution)
            .into_iter()
            .filter(|v| v.constraint_type == ConstraintType::LecturerVeto)
            .collect::<Vec<_>>()
    };

    let breached = report(P0);
    assert_eq!(breached.len(), 1, "P0 at block 0 is one breach: {breached:?}");
    assert!(breached[0].detail.contains("P0"), "must name the Person: {}", breached[0].detail);

    // The same cell with the other candidate is not a breach at all: the
    // answer depends on who was chosen, on the report side as on the filter's.
    assert!(report(P1).is_empty(), "P1 is available at block 0");
}
