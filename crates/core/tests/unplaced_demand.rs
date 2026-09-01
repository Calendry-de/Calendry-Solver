//! ADR-0031: convergence is never declared over unplaced demand.
//!
//! Issue #120's shape: runs of one instance reporting `converged` while
//! leaving Sessions unplaced that other runs of the identical instance DO
//! place. These tests pin the two halves of the fix — the search must resolve
//! an unplaced Session whose only obstacle is a movable neighbour, and a
//! genuinely over-subscribed instance must say `stagnated`, never
//! `converged`.

use calendry_solver_core::search::{Budget, NeverHalt, construct, solve};
use calendry_solver_core::testing;

mod common;
use common::{SEED, solve_to_convergence as run};

#[test]
fn construction_wedges_the_narrow_offering() {
    // The fixture only guards what it claims while greedy really does wedge
    // it: `wide` sorts first and takes the one cell `narrow` could use. If a
    // construction change ever un-wedges it, the tests below pass vacuously
    // and this one says so.
    let problem = testing::evictable_wedge();
    let (solution, _) = construct(&problem);
    assert_eq!(solution.placed_count(), 1, "construction must leave `narrow` unplaced");
}

#[test]
fn an_evictable_wedge_is_resolved_and_reports_converged() {
    // Repair alone can never fix this — an occupied cell scores infinite — so
    // resolving it requires a round that moves the PLACED `wide` for the sake
    // of the UNPLACED `narrow`.
    let problem = testing::evictable_wedge();
    let outcome = run(&problem);

    assert_eq!(outcome.objective.unplaced, 0, "the search must evict `wide` for `narrow`");
    assert!(outcome.hard_violations.is_empty(), "got {:?}", outcome.hard_violations);
    assert_eq!(outcome.termination_reason, "converged");
}

#[test]
fn genuine_oversubscription_reports_stagnated_not_converged() {
    // 4 required Sessions into 3 room-slots: the shortfall is structural. An
    // unbudgeted run must exhaust the escalation ladder and say so — never
    // `converged` over unplaced demand — and must still terminate, which this
    // test proves by finishing (the ladder being finite is what preserves the
    // stagnation limit's original job).
    let problem = testing::oversubscribed();
    let outcome = run(&problem);

    assert_eq!(outcome.solution.placed_count(), 3, "must still fill every real cell");
    assert_eq!(outcome.objective.unplaced, 1);
    assert_eq!(outcome.termination_reason, "stagnated");
}

#[test]
fn a_budget_hit_still_reports_the_budget() {
    // The reasons stay disjoint: a run stopped by its move budget reports the
    // budget, whatever its unplaced count — `stagnated` is only ever the
    // ladder's own verdict.
    let problem = testing::oversubscribed();
    let outcome = solve(&problem, SEED, Budget { max_wall_millis: 0, max_moves: 10 }, &NeverHalt);
    assert_eq!(outcome.termination_reason, "move_budget");
}

#[test]
fn escalated_runs_stay_deterministic() {
    // `stagnated` joins `converged` and `move_budget` in the deterministic
    // set (ADR-0006): same seed, same unbudgeted instance, byte-identical
    // output — even though the run climbed the whole escalation ladder,
    // reheats, escalated ruin caps and the targeted operator included.
    let problem = testing::oversubscribed();
    let first = run(&problem);
    let again = run(&problem);

    let a: Vec<_> = problem
        .placement_ids()
        .map(|p| first.solution.get(p))
        .collect();
    let b: Vec<_> = problem
        .placement_ids()
        .map(|p| again.solution.get(p))
        .collect();
    assert_eq!(a, b, "same seed must give the identical assignment");
    assert_eq!(first.moves_evaluated, again.moves_evaluated);
    assert_eq!(first.iterations, again.iterations);
    assert_eq!(first.termination_reason, again.termination_reason);
}
