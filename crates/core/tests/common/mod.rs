//! Shared setup for the acceptance tests.
//!
//! It exists for one reason: the four test files each had a private function
//! called `run(&Problem)`, and between them those three functions carried
//! **three different budget semantics**. Two passed `Budget::default()`, which
//! leaves *both* axes unbounded so termination falls to the internal stagnation
//! limit; two duplicated a 50,000-move helper. A reader moving between files saw
//! an identical call and could not tell which applied.
//!
//! Worse, the rule that motivates all of it was documented in exactly one of the
//! places obliged to obey it. It is documented here instead, once.
//!
//! Only *setup* is shared. Each test keeps its own action and assertions.

// This module is compiled separately into each of the four test binaries, and
// each uses a different subset of it — so `dead_code` and `unreachable_pub` fire
// on whatever a given binary does not happen to call. That is a property of how
// cargo builds integration tests, not of this file.
#![allow(dead_code, unreachable_pub)]

use calendry_solver_core::Problem;
use calendry_solver_core::search::{Budget, NeverHalt, SolveOutcome, solve};

/// The one seed every acceptance test uses, so a failure is reproducible from
/// the test name alone.
pub const SEED: u64 = 0xC0FFEE;

/// A **move** budget.
///
/// Never a wall-clock budget. Same seed gives byte-identical output only when
/// termination is itself deterministic — `"converged"`, `"stagnated"` or
/// `"move_budget"`. A run stopped by `max_wall_millis` completes however many
/// LNS iterations the machine happened to manage, so a determinism test
/// written against it will look flaky and will waste somebody's afternoon.
/// That is inherent to a time-boxed metaheuristic, not a defect, and it is why
/// `termination_reason` exists.
pub fn moves(n: u64) -> Budget {
    Budget { max_wall_millis: 0, max_moves: n }
}

/// Run until the search declares convergence: **both** budget axes unbounded, so
/// termination comes from the internal stagnation limit alone.
///
/// Correct for the slice 1 and 2 instances, which have a single feasible packing
/// and reach it in a handful of iterations. Deterministic, because the
/// stagnation limit is a pure function of the instance.
pub fn converged() -> Budget {
    Budget::default()
}

/// Solve to convergence. For instances with one feasible packing.
pub fn solve_to_convergence(problem: &Problem) -> SolveOutcome {
    solve(problem, SEED, converged(), &NeverHalt)
}

/// Solve under a 50,000-move budget. For instances where the search must make
/// tradeoffs and "the" answer is a direction rather than an assignment.
pub fn solve_with_move_budget(problem: &Problem) -> SolveOutcome {
    solve(problem, SEED, moves(50_000), &NeverHalt)
}
