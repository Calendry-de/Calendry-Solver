//! Constructive heuristic, then Large Neighborhood Search with simulated
//! annealing acceptance.
//!
//! # Why LNS rather than plain simulated annealing
//!
//! Classic SA proposes one random move per iteration. That would call
//! [`crate::evaluator::MoveEvaluator::score_batch`] with a single move forever,
//! leaving the batched signature — and the future GPU backend it exists for —
//! with nothing to do. LNS's repair step naturally enumerates every eligible
//! `(slot, room)` for a removed Session, which is exactly a batch.
//!
//! SA supplies the acceptance rule, which is what stops ruin-and-recreate from
//! being pure hill climbing.
//!
//! # Determinism
//!
//! Same seed must give byte-identical output, which is not automatic for a
//! probabilistic search. Four disciplines hold it:
//!
//! 1. The RNG is consumed **strictly sequentially** on the decision path — ruin
//!    size, operator choice, removals, acceptance draws. Parallelism never
//!    influences a decision.
//! 2. `rayon` appears only inside `score_batch`, a pure function writing results
//!    **by index**.
//! 3. **No parallel float reductions.** `f64` addition is not associative, so a
//!    parallel sum would differ run to run. Every objective fold is sequential
//!    and in a fixed order.
//! 4. Every argmin **ties-break by index**, never by arrival order.
//!
//! The limit, which is inherent rather than a defect: a run terminated by the
//! **wall-clock budget** cannot be reproducible, because the iteration count
//! depends on machine speed and load. The guarantee is byte-identical output for
//! the same `(input, seed, move budget)` when termination is deterministic —
//! `"converged"`, `"stagnated"` or `"move_budget"`. `termination_reason` tells a
//! caller which case they got.
//!
//! # Termination over unplaced demand (ADR-0031)
//!
//! `"converged"` is reserved for a best solution with **zero unplaced
//! Sessions**. While unplaced demand remains, hitting the stagnation limit
//! escalates instead of terminating — the ruin cap doubles and the temperature
//! is reheated — because a flat counter cannot tell "no improving move exists"
//! from "the right combination was never sampled". The ladder is finite, so an
//! unbudgeted call still terminates; a run that exhausts it stops with
//! `"stagnated"`, the honest "demand is still unplaced and I ran out of ideas,
//! not budget". Scoped to `unplaced` alone, not all of `Objective::hard()`:
//! the aggregate hard terms can be genuinely unsatisfiable by the data
//! (ADR-0025), and a run succeeding while reporting them stays acceptable.
mod construction;
mod objective;
mod repair;
mod ruin;
#[cfg(test)]
mod tests;
mod trial;

pub use construction::construct;
pub use objective::{objectives_agree, recompute_objective, soft_breakdown};
pub use trial::Trial;

use crate::constraints::{self, Violation};
use crate::evaluator::{CpuEvaluator, MoveEvaluator};
use crate::problem::Problem;
use crate::rng::Rng;
use crate::soft::Objective;
use crate::solution::Solution;

use repair::repair_one;
use ruin::ruin;

/// Search hyperparameters.
///
/// These are tuning knobs for the metaheuristic, not domain constants. The ban
/// on magic numbers in this project is about **domain** assumptions — `slot % 3`,
/// `timeslot > 14`, `weeks[-n:]` — which silently encode a grid or calendar the
/// tenant did not configure. A cooling rate encodes nothing about Calendry.
pub mod tuning {
    /// Geometric cooling factor applied once per iteration.
    pub const COOLING: f64 = 0.999;
    /// Temperature floor, below which acceptance is effectively greedy.
    pub const MIN_TEMPERATURE: f64 = 1e-6;
    /// Non-improving iterations tolerated before declaring convergence. Scaled
    /// by instance size so a bigger problem gets proportionally more patience.
    /// Exists so an unbudgeted call terminates rather than spinning forever.
    pub const STAGNATION_BASE: u64 = 200;
    pub const STAGNATION_PER_PLACEMENT: u64 = 20;
    /// Upper bound on candidates scored per repaired Session. A full enumeration
    /// is `slots x eligible_rooms`, which is fine for correctness-sized
    /// instances but wasteful at university scale; beyond this the candidate
    /// list is sampled with the seeded RNG.
    pub const MAX_CANDIDATES: usize = 512;
    /// Base cap on placed Sessions disturbed per ruin round, doubled per
    /// escalation level (ADR-0031).
    pub const RUIN_CAP_BASE: usize = 8;
    /// Escalation levels available while unplaced demand remains. Each level
    /// doubles the ruin cap and reheats the temperature; the ladder being
    /// FINITE is what keeps an unbudgeted call terminating — the same job the
    /// stagnation limit itself exists for. With 3 levels the cap runs
    /// 8 → 16 → 32 → 64.
    pub const ESCALATION_LEVELS: u32 = 3;
    /// Candidate cells probed per targeted-ruin invocation (ADR-0031): for
    /// each, the operator identifies the placed Sessions blocking it and
    /// evicts the cheapest such set.
    pub const BLOCK_PROBE_CELLS: usize = 16;
}

/// Both budgets apply; whichever is hit first ends the run. 0 = unbounded.
#[derive(Copy, Clone, Debug, Default)]
pub struct Budget {
    pub max_wall_millis: u64,
    pub max_moves: u64,
}

#[derive(Clone, Debug)]
pub struct SolveOutcome {
    pub solution: Solution,
    pub objective: Objective,
    pub hard_violations: Vec<Violation>,
    pub moves_evaluated: u64,
    /// Candidate `(slot, room)` pairs **enumerated** by repair, before sampling
    /// down to `tuning::MAX_CANDIDATES`. `moves_evaluated` counts what survived.
    ///
    /// Diagnostic only — never on the wire. The ratio between the two is the
    /// enumeration waste, which is invisible in `moves_evaluated` alone and is
    /// the quantity the benchmark harness exists to expose.
    pub candidates_enumerated: u64,
    pub moves_accepted: u64,
    pub iterations: u64,
    pub termination_reason: &'static str,
}

/// Anything that can stop a run from outside: cancellation, wall-clock budget.
pub trait Halt: Sync {
    fn should_stop(&self) -> Option<&'static str>;
    /// Called with the best-so-far objective so a polling caller can observe
    /// progress. Default: ignore.
    fn report(&self, _objective: f64, _moves: u64) {}
}

pub struct NeverHalt;
impl Halt for NeverHalt {
    fn should_stop(&self) -> Option<&'static str> {
        None
    }
}

/// Optimize with the default CPU move evaluator.
///
/// See [`solve_with`] to supply your own.
pub fn solve(problem: &Problem, seed: u64, budget: Budget, halt: &dyn Halt) -> SolveOutcome {
    solve_with(problem, seed, budget, halt, &CpuEvaluator)
}

/// Optimize, driving `evaluator` for candidate-move scoring.
///
/// The evaluator is a parameter, not a hardcoded local. The trait existed for a
/// deferred GPU backend, but the seam was **not reachable**: `solve` constructed
/// `CpuEvaluator` itself and its signature had no evaluator on it, so swapping a
/// backend meant editing this module — which is exactly what a seam is supposed
/// to make unnecessary. `Halt`, a few lines above, is the shape this copies:
/// parameter on `solve`, three real adapters.
///
/// Generic rather than `&dyn`, so the hot loop dispatches statically.
pub fn solve_with<E: MoveEvaluator>(
    problem: &Problem,
    seed: u64,
    budget: Budget,
    halt: &dyn Halt,
    evaluator: &E,
) -> SolveOutcome {
    let mut rng = Rng::new(seed);

    let mut trial = Trial::construct(problem);
    let mut best = trial.solution().clone();
    let mut best_objective = trial.objective();

    let mut moves_evaluated = 0u64;
    let mut candidates_enumerated = 0u64;
    let mut moves_accepted = 0u64;
    let mut iterations = 0u64;
    let mut termination_reason = "converged";

    let mut temperature = initial_temperature(problem);
    let stagnation_limit = tuning::STAGNATION_BASE
        + tuning::STAGNATION_PER_PLACEMENT * problem.placements.len() as u64;
    let mut stagnant = 0u64;
    // Escalation level (ADR-0031): 0 is the ordinary search. Raised instead of
    // terminating when stagnation hits while Sessions remain unplaced; reset
    // whenever the best-known unplaced count drops.
    let mut level = 0u32;

    // Nothing to improve: no placements, or an already-perfect objective.
    let mut done =
        problem.placements.is_empty() || best_objective.total(problem.hard_penalty) == 0.0;

    while !done {
        if let Some(reason) = halt.should_stop() {
            termination_reason = reason;
            break;
        }
        if budget.max_moves != 0 && moves_evaluated >= budget.max_moves {
            termination_reason = "move_budget";
            break;
        }
        if stagnant >= stagnation_limit {
            // "Converged" is reserved for complete demand (ADR-0031): while
            // Sessions remain unplaced, stagnation cannot tell "no improving
            // move exists" from "the right combination was never sampled", so
            // the search escalates — a bigger ruin cap, a reheated
            // temperature — before it is allowed to give up. The ladder is
            // finite, so an unbudgeted call still terminates; exhausting it
            // reports the honest reason instead of a false convergence.
            if best_objective.unplaced == 0 {
                termination_reason = "converged";
                break;
            }
            if level >= tuning::ESCALATION_LEVELS {
                termination_reason = "stagnated";
                break;
            }
            level += 1;
            stagnant = 0;
            // At MIN_TEMPERATURE acceptance is greedy, and the larger
            // rearrangements the raised cap exists for routinely pass through
            // soft-cost-worse intermediate rounds greed rejects.
            temperature = initial_temperature(problem);
        }

        iterations += 1;

        let before = trial.objective().total(problem.hard_penalty);

        // Everything from here to accept/reject is one recorded round. `begin`
        // marks the journal and snapshots the objective scalars; `rollback`
        // reverses exactly this, in O(k), rather than rebuilding the occupancy.
        trial.begin();

        // --- ruin ---------------------------------------------------------
        let ruin_cap = tuning::RUIN_CAP_BASE << level;
        let removed = ruin(problem, &mut trial, &mut rng, ruin_cap);
        if removed.is_empty() {
            trial.rollback();
            stagnant += 1;
            continue;
        }

        // --- recreate -----------------------------------------------------
        // No hand-maintained deltas: `Trial::place` updates the solution, the
        // occupancy index, the aggregate counters and the objective as one
        // operation, so they cannot disagree.
        for &(p, was_unplaced) in &removed {
            let scored = repair_one(
                problem,
                evaluator,
                trial.state(),
                trial.solution(),
                p,
                was_unplaced,
                &mut rng,
            );
            moves_evaluated += scored.evaluated;
            candidates_enumerated += scored.enumerated;
            if let Some(placement) = scored.best {
                let placed = trial.place(p, placement);
                debug_assert!(
                    placed,
                    "repair proposed a placement whose span does not fit the grid"
                );
            }
        }

        trial.assert_consistent();

        let after = trial.objective().total(problem.hard_penalty);
        let delta = after - before;

        // --- accept -------------------------------------------------------
        let accept = if delta < 0.0 {
            true
        } else if temperature <= tuning::MIN_TEMPERATURE {
            delta == 0.0
        } else {
            // The only RNG draw in acceptance, consumed sequentially.
            let p = (-delta / temperature).exp();
            (rng.next_u64() as f64 / u64::MAX as f64) <= p
        };

        if accept {
            trial.commit();
            moves_accepted += 1;
        } else {
            trial.rollback();
        }

        let objective = trial.objective();
        if objective.total(problem.hard_penalty) < best_objective.total(problem.hard_penalty) {
            if objective.unplaced < best_objective.unplaced {
                // The problem just got smaller: fresh patience at base
                // intensity for what remains (ADR-0031).
                level = 0;
            }
            best = trial.solution().clone();
            best_objective = objective;
            stagnant = 0;
            halt.report(best_objective.total(problem.hard_penalty), moves_evaluated);
        } else {
            stagnant += 1;
        }

        if best_objective.total(problem.hard_penalty) == 0.0 {
            done = true;
        }
        temperature = (temperature * tuning::COOLING).max(tuning::MIN_TEMPERATURE);
    }

    // Canonical value for the returned solution, so callers never see a figure
    // carrying accumulated floating-point drift from the incremental path.
    let best_objective = recompute_objective(problem, &best);

    let hard_violations = constraints::evaluate_hard(problem, &best);

    SolveOutcome {
        solution: best,
        objective: best_objective,
        hard_violations,
        moves_evaluated,
        candidates_enumerated,
        moves_accepted,
        iterations,
        termination_reason,
    }
}

/// Set so a move worsening the objective by the average instance weight is
/// accepted roughly half the time at the start. Derived from the instance
/// rather than tuned.
fn initial_temperature(problem: &Problem) -> f64 {
    // `PersonPreferenceFit` counts here too, or a run whose ONLY soft rule is
    // the preference type would start at `MIN_TEMPERATURE` and hill-climb.
    // Minimize-movement joins them for the same reason: a scope-limited
    // re-solve with no other soft rule configured would otherwise start cold.
    // The in-scope counterpart is a separate axis (issue #58) that can be
    // configured independently, so it gets its own `n`/`total` bump rather
    // than sharing `movement_weight`'s.
    let mut n = problem.soft.instances.len() + problem.preferences.instances.len();
    let mut total = problem.soft.total_weight + problem.preferences.total_weight;
    if problem.movement_weight > 0.0 {
        n += 1;
        total += problem.movement_weight;
    }
    if problem.in_scope_movement_weight > 0.0 {
        n += 1;
        total += problem.in_scope_movement_weight;
    }
    if n == 0 {
        return tuning::MIN_TEMPERATURE;
    }
    let avg = total / n as f64;
    (avg / std::f64::consts::LN_2).max(tuning::MIN_TEMPERATURE)
}
