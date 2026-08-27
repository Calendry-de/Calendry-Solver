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

use std::collections::HashMap;

use crate::constraints::{self, Violation};
use crate::evaluator::{CpuEvaluator, Move, MoveEvaluator, Score};
use crate::ids::PlacementIdx;
use crate::problem::Problem;
use crate::rng::Rng;
use crate::soft::{Objective, RankSpan, SoftComponent};
use crate::solution::{Occupant, Placement, SearchState, Solution};

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

// ---------------------------------------------------------------------------
// Trial
// ---------------------------------------------------------------------------

/// One change to the trial, enough to reverse it.
#[derive(Copy, Clone, Debug)]
enum Change {
    Placed(PlacementIdx, Placement),
    Removed(PlacementIdx, Placement),
}

/// The scalar half of the objective, restored verbatim on rollback.
///
/// Snapshotting rather than replaying the arithmetic backwards is deliberate:
/// `f64` addition is not associative, so `(a - x) + x` is not guaranteed to be
/// `a`, and a rejected round must leave the accepted objective bit-identical.
#[derive(Copy, Clone, Debug)]
struct Scalars {
    unplaced: u32,
    soft: f64,
    journal_len: usize,
}

/// Solution, incremental index and objective, kept in agreement **by
/// construction**.
///
/// `solve` used to hand-maintain the three as siblings, and every mutation had
/// to update all three, in the right order, at four separate sites — with the
/// undo reversing two of them by one mechanism and the third by a different one
/// (not assigning it). The project knew this was fragile: there was a
/// per-iteration `debug_assert` comparing the incremental objective against a
/// full recomputation, and its comment called it "the classic metaheuristic
/// bug". That assertion was the honest admission that the invariant had no
/// owner. It also cost a public interface, because `recompute_objective` and
/// `objectives_agree` had to be exported for tests to re-run the same check.
///
/// Now every primitive updates all three together, so agreement is structural.
/// The drift check remains, as [`Trial::assert_consistent`], but it verifies one
/// module's invariant instead of cross-checking a caller's bookkeeping.
pub struct Trial<'p> {
    problem: &'p Problem,
    solution: Solution,
    state: SearchState,
    unplaced: u32,
    soft: f64,
    /// LIFO record of what the open round changed.
    journal: Vec<Change>,
    open: Option<Scalars>,
}

impl<'p> Trial<'p> {
    /// Greedy construction, with the objective computed once from scratch.
    pub fn construct(problem: &'p Problem) -> Self {
        let (solution, state) = construct(problem);
        let objective = recompute_objective(problem, &solution);
        Self {
            problem,
            solution,
            state,
            unplaced: objective.unplaced,
            soft: objective.soft,
            journal: Vec::new(),
            open: None,
        }
    }

    #[inline]
    pub fn solution(&self) -> &Solution {
        &self.solution
    }

    #[inline]
    pub fn state(&self) -> &SearchState {
        &self.state
    }

    /// The current objective.
    ///
    /// `unplaced` and `soft` are maintained incrementally. `aggregate` and
    /// `day_mix_cost` are read straight off the counters, which ARE the running
    /// state rather than something a delta accumulates into — a violated share
    /// cell and a mixed `(group, day)` cell each belong to no single placement,
    /// so neither can be attributed to one as a delta.
    #[inline]
    pub fn objective(&self) -> Objective {
        Objective {
            unplaced: self.unplaced,
            aggregate: self.state.share_violations(),
            soft: self.soft,
            day_mix_cost: self.state.day_mix_cost(self.problem),
        }
    }

    /// Place `p` at `at`, updating solution, index and objective together.
    ///
    /// Returns `false` and changes nothing if the Session would not fit the grid
    /// there — the case the open-coded ritual used to skip silently while
    /// recording the placement anyway.
    #[must_use = "a false return means nothing was placed"]
    pub fn place(&mut self, p: PlacementIdx, at: Placement) -> bool {
        debug_assert!(self.solution.get(p).is_none(), "place on an occupied placement");
        if !self.state.place(self.problem, p, at) {
            return false;
        }
        self.solution.set(p, Some(at));
        let o = self.problem.offering_of(p);
        self.soft += self.problem.soft.cost(o.soft_profile, at.start, at.room);
        self.unplaced -= 1;
        self.journal.push(Change::Placed(p, at));
        true
    }

    /// Remove `p`, returning where it was. `None` if it was already unplaced,
    /// in which case nothing changed.
    pub fn unplace(&mut self, p: PlacementIdx) -> Option<Placement> {
        let at = self.solution.get(p)?;
        let released = self.state.unplace(self.problem, p, at);
        debug_assert!(released, "a placed Session's span must still resolve");
        self.solution.set(p, None);
        let o = self.problem.offering_of(p);
        self.soft -= self.problem.soft.cost(o.soft_profile, at.start, at.room);
        self.unplaced += 1;
        self.journal.push(Change::Removed(p, at));
        Some(at)
    }

    /// Start recording, so [`Trial::rollback`] can reverse exactly what follows.
    pub fn begin(&mut self) {
        debug_assert!(self.open.is_none(), "a round is already open");
        self.open = Some(Scalars {
            unplaced: self.unplaced,
            soft: self.soft,
            journal_len: self.journal.len(),
        });
    }

    /// Keep everything the open round did.
    pub fn commit(&mut self) {
        let Some(mark) = self.open.take() else { return };
        self.journal.truncate(mark.journal_len);
    }

    /// Reverse the open round exactly, in O(k).
    pub fn rollback(&mut self) {
        let Some(mark) = self.open.take() else { return };
        while self.journal.len() > mark.journal_len {
            // LIFO: undoing in reverse order means the index sees the same
            // sequence of marks it would have seen had the round never run.
            match self.journal.pop() {
                Some(Change::Placed(p, at)) => {
                    let released = self.state.unplace(self.problem, p, at);
                    debug_assert!(released, "rollback of a placement that did not resolve");
                    self.solution.set(p, None);
                }
                Some(Change::Removed(p, at)) => {
                    let marked = self.state.place(self.problem, p, at);
                    debug_assert!(marked, "rollback of a removal that did not resolve");
                    self.solution.set(p, Some(at));
                }
                None => break,
            }
        }
        // Restored verbatim, not recomputed: see [`Scalars`].
        self.unplaced = mark.unplaced;
        self.soft = mark.soft;
    }

    /// The maintained objective must equal a from-scratch recomputation.
    ///
    /// Delta drift is the classic metaheuristic bug — the search optimizes a
    /// number that has quietly diverged from the real objective. Checked on
    /// every iteration in debug builds, and by an explicit test.
    #[inline]
    pub fn assert_consistent(&self) {
        debug_assert!(
            objectives_agree(self.objective(), recompute_objective(self.problem, &self.solution)),
            "incremental objective {:?} diverged from recomputed {:?}",
            self.objective(),
            recompute_objective(self.problem, &self.solution)
        );
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
            termination_reason = "converged";
            break;
        }

        iterations += 1;

        let before = trial.objective().total(problem.hard_penalty);

        // Everything from here to accept/reject is one recorded round. `begin`
        // marks the journal and snapshots the objective scalars; `rollback`
        // reverses exactly this, in O(k), rather than rebuilding the occupancy.
        trial.begin();

        // --- ruin ---------------------------------------------------------
        let removed = ruin(problem, &mut trial, &mut rng);
        if removed.is_empty() {
            trial.rollback();
            stagnant += 1;
            continue;
        }

        // --- recreate -----------------------------------------------------
        // No hand-maintained deltas: `Trial::place` updates the solution, the
        // occupancy index, the aggregate counters and the objective as one
        // operation, so they cannot disagree.
        for &p in &removed {
            let scored =
                repair_one(problem, evaluator, trial.state(), trial.solution(), p, &mut rng);
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

        // Testing the room-independent axes ONCE per slot, before the room loop,
        // is a pure short-circuit: if they reject, no Room can rescue the slot,
        // so the loop that follows could only have failed. Measured, ~60% of
        // start slots are rejected this way, and the saving is larger than that
        // count suggests — the room check is a single early-exiting bit test,
        // while the room-independent path scans an attendee list averaging 65
        // people. Previously that scan ran once per *free* Room per slot.
        //
        // The mask itself lives on `Occupant`, because the benchmark harness's
        // construction attribution has to use the identical one to report
        // truthfully. See [`Occupant::room_independent_probe`].
        let slot_probe = Occupant::room_independent_probe(offering);

        let mut chosen = None;
        'search: for slot in problem.slots.all() {
            let Some(span) = problem.slots.span(slot, offering.duration_blocks) else {
                continue;
            };
            if let Some(probe) = slot_probe.as_ref()
                && !state.is_free(problem, probe, &span)
            {
                continue;
            }
            for &room in &offering.eligible_rooms {
                let candidate = base.with_room(room);
                if state.is_free(problem, &candidate, &span) {
                    chosen = Some(Placement { start: slot, room });
                    break 'search;
                }
            }
        }

        // Leaving a placement unplaced is a legitimate outcome: it surfaces as
        // an ExactFrequency violation rather than an error, because the solver
        // must degrade gracefully on infeasible input.
        if let Some(placement) = chosen {
            let marked = state.place(problem, p, placement);
            debug_assert!(marked, "construction chose a placement whose span resolved");
            solution.set(p, Some(placement));
        }
    }

    (solution, state)
}

// ---------------------------------------------------------------------------
// Ruin
// ---------------------------------------------------------------------------

/// Remove a handful of placements, releasing their occupancy.
///
/// Three operators, chosen by the seeded RNG. `Related` is what lets the search
/// *swap* two Sessions: any one-at-a-time neighbourhood has to pass through an
/// infeasible intermediate to reach a swap, so without it those moves are
/// unreachable.
///
/// The removed positions no longer come back as a second return value: the
/// `Trial`'s journal records them, so the undo is its business rather than the
/// caller's.
fn ruin(problem: &Problem, trial: &mut Trial<'_>, rng: &mut Rng) -> Vec<PlacementIdx> {
    // Selection reads the solution; removal mutates the trial. Scoped so the
    // shared borrow ends before the exclusive one begins.
    let chosen = {
        let current = trial.solution();
        let placed: Vec<PlacementIdx> = problem
            .placement_ids()
            .filter(|&p| current.get(p).is_some())
            .collect();

        // Anything construction failed to place is retried on every iteration.
        // Without this, ruin only ever selects PLACED Sessions, so a Session
        // that greedy dead-ended on could never be reconsidered and the
        // `unplaced` term of the objective would be permanently unoptimizable.
        let unplaced: Vec<PlacementIdx> = problem
            .placement_ids()
            .filter(|&p| current.get(p).is_none())
            .collect();

        if placed.is_empty() {
            // Nothing to release; the unplaced simply join the repair list.
            unplaced
        } else {
            // Ruin size: at least 1, at most 8 or the number placed, whichever
            // is smaller.
            let max_k = placed.len().clamp(1, 8);
            let k = 1 + rng.below(max_k);

            let mut chosen = match rng.below(3) {
                0 => ruin_random(&placed, k, rng),
                1 => ruin_worst(problem, current, &placed, k),
                _ => ruin_related(problem, current, &placed, k, rng),
            };
            chosen.extend_from_slice(&unplaced);
            chosen.sort_unstable();
            chosen.dedup();
            chosen
        }
    };

    for &p in &chosen {
        // Already-unplaced entries return `None` and change nothing.
        let _ = trial.unplace(p);
    }
    chosen
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
    /// Post-sampling: what `score_batch` actually saw.
    evaluated: u64,
    /// Pre-sampling: the full `slots x eligible_rooms` cross product.
    enumerated: u64,
}

/// Score every eligible `(slot, room)` for one removed Session as a batch, and
/// take the cheapest feasible one.
fn repair_one<E: MoveEvaluator>(
    problem: &Problem,
    evaluator: &E,
    state: &SearchState,
    solution: &Solution,
    p: PlacementIdx,
    rng: &mut Rng,
) -> Repaired {
    let offering = problem.offering_of(p);
    let n_rooms = offering.eligible_rooms.len();
    let n_starts = problem.slots.start_count(offering.duration_blocks);
    let total = n_starts * n_rooms;
    if total == 0 {
        return Repaired { best: None, evaluated: 0, enumerated: 0 };
    }
    let enumerated = total as u64;

    // The candidate space is addressed BY INDEX, never materialized.
    //
    // Index `i` is slot-major: `(nth_start(i / n_rooms), eligible_rooms[i %
    // n_rooms])`, which is the order a nested slot-then-room loop would produce.
    let at = |i: usize| Move {
        placement: p,
        to: Placement {
            start: problem
                .slots
                .nth_start(offering.duration_blocks, i / n_rooms)
                .expect("index below start_count"),
            room: offering.eligible_rooms[i % n_rooms],
        },
    };

    let keep = total.min(tuning::MAX_CANDIDATES);
    let mut candidates: Vec<Move> = Vec::with_capacity(keep);

    if total <= tuning::MAX_CANDIDATES {
        candidates.extend((0..total).map(at));
    } else {
        // Partial Fisher-Yates over a VIRTUAL array [0, total).
        //
        // Building the real array first cost `starts x eligible_rooms` pushes to
        // keep 512 of them — 65% of repair time at large-university scale, and
        // 99.4% of the work discarded. `moved` records only the positions the
        // shuffle actually disturbed, which is O(keep), not O(total).
        //
        // The RNG is consumed in exactly the same sequence as the materializing
        // version, and the virtual array's element at index i is `at(i)`, so this
        // selects the identical subset. The change is purely one of cost: same
        // seed still gives byte-identical output.
        let mut moved: HashMap<usize, usize> = HashMap::with_capacity(keep);
        for i in 0..keep {
            let j = i + rng.below(total - i);
            let picked = moved.get(&j).copied().unwrap_or(j);
            let displaced = moved.get(&i).copied().unwrap_or(i);
            candidates.push(at(picked));
            // Position i is finalized once visited and never read again, so only
            // position j needs recording.
            moved.insert(j, displaced);
        }
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
    for s in &scores {
        if s.0.is_finite() && s.0 < best_score {
            best_score = s.0;
        }
    }
    if !best_score.is_finite() {
        return Repaired { best: None, evaluated: candidates.len() as u64, enumerated };
    }

    let tied: Vec<usize> = scores
        .iter()
        .enumerate()
        .filter(|(_, s)| s.0 <= best_score + f64::EPSILON)
        .map(|(i, _)| i)
        .collect();
    let pick = tied[rng.below(tied.len())];

    Repaired { best: Some(candidates[pick].to), evaluated: candidates.len() as u64, enumerated }
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
    let state = SearchState::replay(problem, solution);

    Objective {
        unplaced,
        aggregate: state.share_violations(),
        soft,
        day_mix_cost: state.day_mix_cost(problem),
    }
}

/// Per-instance counts for `ObjectiveBreakdown`.
///
/// Recomputed from scratch at the end of a run using the **same predicate** the
/// cost table was built from, so the fast path and the reported counts cannot
/// disagree.
pub fn soft_breakdown(problem: &Problem, solution: &Solution) -> Vec<SoftComponent> {
    /*
     * The day-mix instances come first and separately, because they are not in
     * `problem.soft` — see `ConstraintSet::online_onsite_same_day`. Reported
     * here rather than as a hard violation: since the reclassification a mixed
     * day is a priced outcome, and the breakdown is the place a human is shown
     * what the score is made of.
     *
     * `raw_count` is the mixed CELL count, which is the question somebody
     * actually asks ("how many group-days ended up mixed?"), and `weighted` is
     * exactly what the objective charged for them.
     */
    let state = SearchState::replay(problem, solution);
    let mixed_cells = state.aggregates.day_mix_violations() as u64;

    let day_mix = problem
        .constraints
        .online_onsite_same_day
        .iter()
        .map(|inst| SoftComponent {
            constraint_id: inst.id.clone(),
            constraint_type: constraints::ConstraintType::OnlineOnsiteSameDay.as_str(),
            raw_count: mixed_cells,
            weighted: mixed_cells as f64 * inst.weight,
        });

    day_mix
        .chain(problem.soft.instances.iter().map(|inst| {
            let mut count = 0u64;
            let mut weighted = 0.0f64;
            let ranks = RankSpan::of(&problem.rooms);

            for p in problem.placement_ids() {
                let Some(pl) = solution.get(p) else { continue };
                let o = problem.offering_of(p);
                if !inst.covers(&o.kind) {
                    continue;
                }
                let flags = problem.slots.flags(pl.start);
                let room = &problem.rooms[pl.room.get()];

                if inst.params.applies(flags, room) {
                    count += 1;
                }

                /*
                 * ACCUMULATED, not `count * weight`.
                 *
                 * `MinimizeRoomRank` now grades its penalty by how far past the
                 * threshold a room sits, so a flat multiplication would report a
                 * number the objective does not contain — and this breakdown is
                 * what the app shows a human to explain the score. `severity`
                 * returns 0.0 where the rule does not apply, so this sums exactly
                 * the same cells the cost table charged for.
                 *
                 * `raw_count` deliberately stays a COUNT: "sessions in
                 * discouraged rooms" is the question a person asks, and it is
                 * still answered by the same predicate.
                 */
                weighted += inst.weight * inst.params.severity(flags, room, ranks);
            }

            SoftComponent {
                constraint_id: inst.id.clone(),
                constraint_type: inst.params.type_name(),
                raw_count: count,
                weighted,
            }
        }))
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
        && (a.day_mix_cost - b.day_mix_cost).abs() <= 1e-9 * (1.0 + a.day_mix_cost.abs())
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
