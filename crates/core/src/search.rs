//! The constructive heuristic.
//!
//! Greedy construction only. No simulated annealing, no LNS — those arrive in a
//! later slice, driving [`crate::evaluator::MoveEvaluator`]. The budget plumbing
//! is real now so that termination semantics were settled before a metaheuristic
//! started consuming them.

use crate::constraints::{self, Violation};
use crate::ids::PlacementIdx;
use crate::problem::Problem;
use crate::rng::Rng;
use crate::solution::{Occupancy, Occupant, Placement, Solution};

/// Both budgets apply; whichever is hit first ends the run. 0 = unbounded.
#[derive(Copy, Clone, Debug, Default)]
pub struct Budget {
    pub max_wall_millis: u64,
    pub max_moves: u64,
}

#[derive(Clone, Debug)]
pub struct SolveOutcome {
    pub solution: Solution,
    pub hard_violations: Vec<Violation>,
    pub moves_evaluated: u64,
    pub moves_accepted: u64,
    pub termination_reason: &'static str,
}

/// Anything that can stop a run from outside: cancellation, wall-clock budget.
pub trait Halt: Sync {
    fn should_stop(&self) -> Option<&'static str>;
}

pub struct NeverHalt;
impl Halt for NeverHalt {
    fn should_stop(&self) -> Option<&'static str> {
        None
    }
}

pub fn solve(problem: &Problem, seed: u64, budget: Budget, halt: &dyn Halt) -> SolveOutcome {
    let mut rng = Rng::new(seed);
    let mut solution = Solution::empty(problem);
    let mut occupancy = Occupancy::from_fixed(problem);

    let mut moves_evaluated = 0u64;
    let mut moves_accepted = 0u64;
    let mut termination_reason = "converged";

    'placements: for p in placement_order(problem, &mut rng) {
        if let Some(reason) = halt.should_stop() {
            termination_reason = reason;
            break;
        }
        if budget.max_moves != 0 && moves_evaluated >= budget.max_moves {
            termination_reason = "move_budget";
            break;
        }

        let offering = problem.offering_of(p);
        let base = Occupant::of_offering(offering);
        let mut chosen = None;

        'search: for slot in problem.slots.all() {
            let Some(span) = problem.slots.span(slot, offering.duration_blocks) else {
                continue;
            };
            for &room in &offering.eligible_rooms {
                moves_evaluated += 1;
                let candidate = base.with_room(room);
                if occupancy.is_free(&candidate, &span) {
                    chosen = Some((Placement { start: slot, room }, span, candidate));
                    break 'search;
                }
                if budget.max_moves != 0 && moves_evaluated >= budget.max_moves {
                    termination_reason = "move_budget";
                    break 'search;
                }
            }
        }

        // Leaving a placement unplaced is a legitimate outcome: it surfaces as
        // an ExactFrequency violation rather than an error, because the solver
        // must degrade gracefully on infeasible input.
        if let Some((placement, span, occupant)) = chosen {
            occupancy.mark(&occupant, &span);
            solution.set(p, Some(placement));
            moves_accepted += 1;
        }

        if termination_reason == "move_budget" {
            break 'placements;
        }
    }

    let hard_violations = constraints::evaluate_hard(problem, &solution);

    SolveOutcome {
        solution,
        hard_violations,
        moves_evaluated,
        moves_accepted,
        termination_reason,
    }
}

/// Most-constrained-first, with seeded tie-breaking.
///
/// Ordering is a pure function of (problem, seed), so the same pair always
/// produces the same schedule.
fn placement_order(problem: &Problem, rng: &mut Rng) -> Vec<PlacementIdx> {
    let mut order: Vec<PlacementIdx> = problem.placement_ids().collect();

    // Deterministic tie-break key drawn from the seeded stream, one per
    // placement, assigned in index order.
    let keys: Vec<u64> = (0..order.len()).map(|_| rng.next_u64()).collect();

    order.sort_by(|&a, &b| {
        let oa = problem.offering_of(a);
        let ob = problem.offering_of(b);
        // Fewer eligible rooms first, then more attendees (harder to place),
        // then longer sessions first.
        oa.eligible_rooms
            .len()
            .cmp(&ob.eligible_rooms.len())
            .then(ob.attendees.len().cmp(&oa.attendees.len()))
            .then(ob.duration_blocks.cmp(&oa.duration_blocks))
            .then(keys[a.get()].cmp(&keys[b.get()]))
            .then(a.cmp(&b))
    });
    order
}
