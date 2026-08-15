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
//! `"converged"` or `"move_budget"`. `termination_reason` tells a caller which
//! case they got.

use crate::constraints::{self, Violation};
use crate::evaluator::{CpuEvaluator, Move, MoveEvaluator, Score};
use crate::ids::{PlacementIdx, RoomIdx};
use crate::problem::Problem;
use crate::rng::Rng;
use crate::soft::{Objective, SoftComponent};
use crate::solution::{Occupant, Placement, SearchState, Solution};

/// Search hyperparameters.
///
/// These are tuning knobs for the metaheuristic, not domain constants. The ban
/// on magic numbers in this project is about **domain** assumptions — `slot % 3`,
/// `timeslot > 14`, `weeks[-n:]` — which silently encode a grid or calendar the
/// tenant did not configure. A cooling rate encodes nothing about Calendry.
mod tuning {
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

pub fn solve(problem: &Problem, seed: u64, budget: Budget, halt: &dyn Halt) -> SolveOutcome {
    let mut rng = Rng::new(seed);

    let (mut current, mut state) = construct(problem);
    let mut objective = recompute_objective(problem, &current);

    let mut best = current.clone();
    let mut best_objective = objective;

    let mut moves_evaluated = 0u64;
    let mut moves_accepted = 0u64;
    let mut iterations = 0u64;
    let mut termination_reason = "converged";

    let evaluator = CpuEvaluator;
    let mut temperature = initial_temperature(problem);
    let stagnation_limit = tuning::STAGNATION_BASE
        + tuning::STAGNATION_PER_PLACEMENT * problem.placements.len() as u64;
    let mut stagnant = 0u64;

    // Nothing to improve: no placements, or an already-perfect objective.
    let mut done = problem.placements.is_empty()
        || best_objective.total(problem.hard_penalty) == 0.0;

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
            termination_reason = "converged";
            break;
        }

        iterations += 1;

        // --- ruin ---------------------------------------------------------
        // `original` is what an undo needs: the removed placements and where
        // they were, so a rejected trial is rolled back in O(k) rather than by
        // rebuilding the whole occupancy.
        let (removed, original) = ruin(problem, &current, &mut state, &mut rng);
        if removed.is_empty() {
            stagnant += 1;
            continue;
        }

        // Objective is maintained INCREMENTALLY. Every removal subtracts its
        // soft cost and adds an unplaced; every repair does the reverse. A full
        // recomputation is O(placements) and would dominate the loop.
        let mut trial_obj = objective;
        for &(p, pl) in &original {
            let o = problem.offering_of(p);
            trial_obj.soft -= problem.soft.cost(o.soft_profile, pl.start, pl.room);
            trial_obj.unplaced += 1;
        }

        let mut trial = current.clone();
        for &p in &removed {
            trial.set(p, None);
        }

        // --- recreate -----------------------------------------------------
        let mut repaired: Vec<(PlacementIdx, Placement)> = Vec::with_capacity(removed.len());
        for &p in &removed {
            let scored = repair_one(problem, &evaluator, &state, &trial, p, &mut rng);
            moves_evaluated += scored.evaluated;
            if let Some(placement) = scored.best {
                let offering = problem.offering_of(p);
                let occupant = Occupant::of_offering(offering).with_room(placement.room);
                if let Some(span) = problem.slots.span(placement.start, offering.duration_blocks) {
                    state.mark(problem, &occupant, &span);
                }
                trial.set(p, Some(placement));
                trial_obj.soft +=
                    problem.soft.cost(offering.soft_profile, placement.start, placement.room);
                trial_obj.unplaced -= 1;
                repaired.push((p, placement));
            }
        }

        // The aggregate term is read straight off the incremental counters,
        // which `SearchState::mark`/`unmark` have already updated. Unlike the
        // soft sum there is no delta to accumulate — the counters ARE the
        // running state — but they can still drift, which is what the assertion
        // below and the aggregate-drift test exist to catch.
        trial_obj.aggregate = state.share_violations();

        // Delta drift is the classic metaheuristic bug: the search optimizes a
        // number that has quietly diverged from the real objective. Checked on
        // every iteration in debug builds, and by an explicit test.
        debug_assert!(
            objectives_agree(trial_obj, recompute_objective(problem, &trial)),
            "incremental objective {trial_obj:?} diverged from recomputed {:?}",
            recompute_objective(problem, &trial)
        );

        let delta =
            trial_obj.total(problem.hard_penalty) - objective.total(problem.hard_penalty);

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
            current = trial;
            objective = trial_obj;
            moves_accepted += 1;
        } else {
            // Exact O(k) undo: drop what repair placed, restore what ruin took.
            for &(p, pl) in &repaired {
                let o = problem.offering_of(p);
                let occupant = Occupant::of_offering(o).with_room(pl.room);
                if let Some(span) = problem.slots.span(pl.start, o.duration_blocks) {
                    state.unmark(problem, &occupant, &span);
                }
            }
            for &(p, pl) in &original {
                let o = problem.offering_of(p);
                let occupant = Occupant::of_offering(o).with_room(pl.room);
                if let Some(span) = problem.slots.span(pl.start, o.duration_blocks) {
                    state.mark(problem, &occupant, &span);
                }
            }
        }

        if objective.total(problem.hard_penalty) < best_objective.total(problem.hard_penalty) {
            best = current.clone();
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
        moves_accepted,
        iterations,
        termination_reason,
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

/// Greedy construction: most-constrained-first, first feasible slot.
pub fn construct(problem: &Problem) -> (Solution, SearchState) {
    let mut solution = Solution::empty(problem);
    let mut state = SearchState::from_fixed(problem);

    // Ordering is a pure function of the problem; the seed only ever perturbs
    // the LNS phase, so construction is reproducible on its own.
    let mut order: Vec<PlacementIdx> = problem.placement_ids().collect();
    order.sort_by(|&a, &b| {
        let (oa, ob) = (problem.offering_of(a), problem.offering_of(b));
        oa.eligible_rooms
            .len()
            .cmp(&ob.eligible_rooms.len())
            .then(ob.attendees.len().cmp(&oa.attendees.len()))
            .then(ob.duration_blocks.cmp(&oa.duration_blocks))
            .then(a.cmp(&b))
    });

    for p in order {
        let offering = problem.offering_of(p);
        let base = Occupant::of_offering(offering);

        let mut chosen = None;
        'search: for slot in problem.slots.all() {
            let Some(span) = problem.slots.span(slot, offering.duration_blocks) else {
                continue;
            };
            for &room in &offering.eligible_rooms {
                let candidate = base.with_room(room);
                if state.is_free(problem, &candidate, &span) {
                    chosen = Some((Placement { start: slot, room }, span, candidate));
                    break 'search;
                }
            }
        }

        // Leaving a placement unplaced is a legitimate outcome: it surfaces as
        // an ExactFrequency violation rather than an error, because the solver
        // must degrade gracefully on infeasible input.
        if let Some((placement, span, occupant)) = chosen {
            state.mark(problem, &occupant, &span);
            solution.set(p, Some(placement));
        }
    }

    (solution, state)
}

// ---------------------------------------------------------------------------
// Ruin
// ---------------------------------------------------------------------------

/// Remove a handful of placements, unmarking their occupancy.
///
/// Three operators, chosen by the seeded RNG. `Related` is what lets the search
/// *swap* two Sessions: any one-at-a-time neighbourhood has to pass through an
/// infeasible intermediate to reach a swap, so without it those moves are
/// unreachable.
type Ruined = (Vec<PlacementIdx>, Vec<(PlacementIdx, Placement)>);

fn ruin(
    problem: &Problem,
    current: &Solution,
    state: &mut SearchState,
    rng: &mut Rng,
) -> Ruined {
    let placed: Vec<PlacementIdx> = problem
        .placement_ids()
        .filter(|&p| current.get(p).is_some())
        .collect();

    // Anything construction failed to place is retried on every iteration.
    // Without this, ruin only ever selects PLACED Sessions, so a Session that
    // greedy dead-ended on could never be reconsidered and the `unplaced` term
    // of the objective would be permanently unoptimizable.
    let unplaced: Vec<PlacementIdx> = problem
        .placement_ids()
        .filter(|&p| current.get(p).is_none())
        .collect();

    if placed.is_empty() && unplaced.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if placed.is_empty() {
        return (unplaced, Vec::new());
    }

    // Ruin size: at least 1, at most 8 or the number placed, whichever is smaller.
    let max_k = placed.len().clamp(1, 8);
    let k = 1 + rng.below(max_k);

    let chosen = match rng.below(3) {
        0 => ruin_random(&placed, k, rng),
        1 => ruin_worst(problem, current, &placed, k),
        _ => ruin_related(problem, current, &placed, k, rng),
    };

    // Unplaced ones carry no occupancy to release and no original position to
    // restore; they simply join the repair list.
    let mut chosen = chosen;
    chosen.extend_from_slice(&unplaced);
    chosen.sort_unstable();
    chosen.dedup();

    let mut original = Vec::with_capacity(chosen.len());
    for &p in &chosen {
        if let Some(pl) = current.get(p) {
            let offering = problem.offering_of(p);
            let occupant = Occupant::of_offering(offering).with_room(pl.room);
            if let Some(span) = problem.slots.span(pl.start, offering.duration_blocks) {
                state.unmark(problem, &occupant, &span);
            }
            original.push((p, pl));
        }
    }
    (chosen, original)
}

fn ruin_random(placed: &[PlacementIdx], k: usize, rng: &mut Rng) -> Vec<PlacementIdx> {
    let mut pool = placed.to_vec();
    let mut out = Vec::with_capacity(k);
    for _ in 0..k.min(pool.len()) {
        let i = rng.below(pool.len());
        out.push(pool.swap_remove(i));
    }
    out.sort_unstable();
    out
}

/// The placements contributing the most soft penalty.
fn ruin_worst(
    problem: &Problem,
    current: &Solution,
    placed: &[PlacementIdx],
    k: usize,
) -> Vec<PlacementIdx> {
    let mut scored: Vec<(PlacementIdx, f64)> = placed
        .iter()
        .map(|&p| {
            let pl = current.get(p).unwrap();
            let o = problem.offering_of(p);
            (p, problem.soft.cost(o.soft_profile, pl.start, pl.room))
        })
        .collect();
    // Descending cost, ties by index so the choice is deterministic.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out: Vec<PlacementIdx> = scored.into_iter().take(k).map(|(p, _)| p).collect();
    out.sort_unstable();
    out
}

/// A seed placement plus others sharing a room, lecturer or group with it.
fn ruin_related(
    problem: &Problem,
    current: &Solution,
    placed: &[PlacementIdx],
    k: usize,
    rng: &mut Rng,
) -> Vec<PlacementIdx> {
    let anchor = placed[rng.below(placed.len())];
    let anchor_pl = current.get(anchor).unwrap();
    let anchor_o = problem.offering_of(anchor);

    let mut related: Vec<PlacementIdx> = placed
        .iter()
        .copied()
        .filter(|&p| {
            if p == anchor {
                return false;
            }
            let pl = current.get(p).unwrap();
            let o = problem.offering_of(p);
            pl.room == anchor_pl.room
                || o.lecturers.iter().any(|l| anchor_o.lecturers.contains(l))
                || o.own_groups.iter().any(|g| anchor_o.own_groups.contains(g))
        })
        .collect();

    let mut out = vec![anchor];
    for _ in 1..k {
        if related.is_empty() {
            break;
        }
        let i = rng.below(related.len());
        out.push(related.swap_remove(i));
    }
    out.sort_unstable();
    out.dedup();
    out
}

// ---------------------------------------------------------------------------
// Recreate
// ---------------------------------------------------------------------------

struct Repaired {
    best: Option<Placement>,
    evaluated: u64,
}

/// Score every eligible `(slot, room)` for one removed Session as a batch, and
/// take the cheapest feasible one.
fn repair_one(
    problem: &Problem,
    evaluator: &dyn MoveEvaluator,
    state: &SearchState,
    solution: &Solution,
    p: PlacementIdx,
    rng: &mut Rng,
) -> Repaired {
    let offering = problem.offering_of(p);
    if offering.eligible_rooms.is_empty() {
        return Repaired { best: None, evaluated: 0 };
    }

    let mut candidates: Vec<Move> = Vec::new();
    for slot in problem.slots.all() {
        if problem.slots.span(slot, offering.duration_blocks).is_none() {
            continue;
        }
        for &room in &offering.eligible_rooms {
            candidates.push(Move { placement: p, to: Placement { start: slot, room } });
        }
    }
    if candidates.is_empty() {
        return Repaired { best: None, evaluated: 0 };
    }

    // Sample when the enumeration is large. Seeded partial Fisher-Yates, so the
    // subset is a pure function of the RNG stream.
    if candidates.len() > tuning::MAX_CANDIDATES {
        for i in 0..tuning::MAX_CANDIDATES {
            let j = i + rng.below(candidates.len() - i);
            candidates.swap(i, j);
        }
        candidates.truncate(tuning::MAX_CANDIDATES);
        // Restore a canonical order so argmin ties break identically.
        candidates.sort_by_key(|m| (m.to.start.0, m.to.room.0));
    }

    let mut scores = vec![Score::default(); candidates.len()];
    evaluator.score_batch(problem, solution, state, &candidates, &mut scores);

    // Best score, then a SEEDED choice among everything tied with it.
    //
    // Breaking ties by lowest index instead would make repair fully
    // deterministic given a candidate list — and that collapses the
    // neighbourhood: ruining the same Session always regenerates the same
    // placement, so LNS can never escape a tie-induced dead end. (Observed for
    // real: a Group forced onto one day would keep re-picking the virtual room
    // and leave its second Session permanently unplaced.) The RNG is consumed
    // sequentially here like everywhere else, so the run stays reproducible.
    let mut best_score = f64::INFINITY;
    for s in scores.iter() {
        if s.0.is_finite() && s.0 < best_score {
            best_score = s.0;
        }
    }
    if !best_score.is_finite() {
        return Repaired { best: None, evaluated: candidates.len() as u64 };
    }

    let tied: Vec<usize> = scores
        .iter()
        .enumerate()
        .filter(|(_, s)| s.0 <= best_score + f64::EPSILON)
        .map(|(i, _)| i)
        .collect();
    let pick = tied[rng.below(tied.len())];

    Repaired {
        best: Some(candidates[pick].to),
        evaluated: candidates.len() as u64,
    }
}

// ---------------------------------------------------------------------------
// Objective
// ---------------------------------------------------------------------------

/// Full recomputation, in placement-index order so the `f64` fold is
/// bit-reproducible.
pub fn recompute_objective(problem: &Problem, solution: &Solution) -> Objective {
    let mut unplaced = 0u32;
    let mut soft = 0.0f64;
    for p in problem.placement_ids() {
        match solution.get(p) {
            Some(pl) => {
                let o = problem.offering_of(p);
                soft += problem.soft.cost(o.soft_profile, pl.start, pl.room);
            }
            None => unplaced += 1,
        }
    }
    // Aggregate violations are recomputed by replaying the whole solution into
    // a fresh counter set — the from-scratch counterpart to the incremental
    // counters the search maintains.
    let aggregate = rebuild_state(problem, solution).share_violations();
    Objective { unplaced, aggregate, soft }
}

/// Replay a solution into a fresh [`SearchState`]. Used by the from-scratch
/// objective and by the per-iteration drift assertion.
pub fn rebuild_state(problem: &Problem, solution: &Solution) -> SearchState {
    let mut state = SearchState::from_fixed(problem);
    for p in problem.placement_ids() {
        if let Some(pl) = solution.get(p) {
            let o = problem.offering_of(p);
            let occupant = Occupant::of_offering(o).with_room(pl.room);
            if let Some(span) = problem.slots.span(pl.start, o.duration_blocks) {
                state.mark(problem, &occupant, &span);
            }
        }
    }
    state
}

/// Per-instance counts for `ObjectiveBreakdown`.
///
/// Recomputed from scratch at the end of a run using the **same predicate** the
/// cost table was built from, so the fast path and the reported counts cannot
/// disagree.
pub fn soft_breakdown(problem: &Problem, solution: &Solution) -> Vec<SoftComponent> {
    problem
        .soft
        .instances
        .iter()
        .map(|inst| {
            let mut count = 0u64;
            for p in problem.placement_ids() {
                let Some(pl) = solution.get(p) else { continue };
                let o = problem.offering_of(p);
                if !inst.covers(&o.kind) {
                    continue;
                }
                if inst
                    .params
                    .applies(problem.slots.flags(pl.start), &problem.rooms[pl.room.get()])
                {
                    count += 1;
                }
            }
            SoftComponent {
                constraint_id: inst.id.clone(),
                constraint_type: inst.params.type_name(),
                raw_count: count,
                weighted: count as f64 * inst.weight,
            }
        })
        .collect()
}

/// Incremental and recomputed objectives must agree.
///
/// Compared with a tolerance rather than bit-exactly: `f64` addition is not
/// associative, so accumulating deltas and summing from scratch can legitimately
/// differ in the last place. Anything beyond that is drift, and drift is a bug.
pub fn objectives_agree(a: Objective, b: Objective) -> bool {
    a.unplaced == b.unplaced
        && a.aggregate == b.aggregate
        && (a.soft - b.soft).abs() <= 1e-9 * (1.0 + a.soft.abs())
}

/// Set so a move worsening the objective by the average instance weight is
/// accepted roughly half the time at the start. Derived from the instance
/// rather than tuned.
fn initial_temperature(problem: &Problem) -> f64 {
    if problem.soft.is_empty() {
        return tuning::MIN_TEMPERATURE;
    }
    let avg = problem.soft.total_weight / problem.soft.instances.len() as f64;
    (avg / std::f64::consts::LN_2).max(tuning::MIN_TEMPERATURE)
}

/// The rooms a placement could legally use, for diagnostics and tests.
pub fn eligible_rooms(problem: &Problem, p: PlacementIdx) -> &[RoomIdx] {
    &problem.offering_of(p).eligible_rooms
}
