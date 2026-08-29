//! The move-evaluation boundary.
//!
//! This trait is the one concession to the deferred GPU backend. It is
//! **batched by construction**: a per-move `score(&Move) -> Score` signature
//! would make a GPU implementation pointless, since the entire value there is
//! scoring thousands of LNS candidate moves per dispatch. Keeping the batch in
//! the signature means a future backend plugs in without the search changing.
//!
//! v1 ships only [`CpuEvaluator`]. No metaheuristic drives this yet — the
//! constructive heuristic places greedily — but the boundary exists and is
//! exercised, rather than being an empty promise.

use rayon::prelude::*;

use crate::ids::PlacementIdx;
use crate::problem::Problem;
use crate::solution::{Occupant, Placement, SearchState, Solution};

/// A candidate relocation of one placement.
#[derive(Copy, Clone, Debug)]
pub struct Move {
    pub placement: PlacementIdx,
    pub to: Placement,
}

/// Lower is better. `INFINITY` means the move is infeasible — the target is
/// occupied, the room is ineligible, or the Session would spill past the end of
/// its day. Finite scores are the move's **soft cost**, read from the
/// precomputed table, which is exact rather than an estimate because all six
/// soft types are unary.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd, Default)]
pub struct Score(pub f64);

pub trait MoveEvaluator: Send + Sync {
    fn score_batch(
        &self,
        problem: &Problem,
        solution: &Solution,
        state: &SearchState,
        moves: &[Move],
        out: &mut [Score],
    );
}

#[derive(Clone, Debug, Default)]
pub struct CpuEvaluator;

impl MoveEvaluator for CpuEvaluator {
    fn score_batch(
        &self,
        problem: &Problem,
        solution: &Solution,
        state: &SearchState,
        moves: &[Move],
        out: &mut [Score],
    ) {
        assert_eq!(moves.len(), out.len(), "moves and out must be equal length");

        out.par_iter_mut()
            .zip(moves.par_iter())
            .for_each(|(slot, mv)| {
                *slot = score_one(problem, solution, state, mv);
            });
    }
}

fn score_one(problem: &Problem, solution: &Solution, state: &SearchState, mv: &Move) -> Score {
    let offering = problem.offering_of(mv.placement);

    let Some(span) = problem.slots.span(mv.to.start, offering.duration_blocks) else {
        // Would spill past the end of its day: not representable.
        return Score(f64::INFINITY);
    };
    if !offering.eligible_rooms.contains(&mv.to.room) {
        return Score(f64::INFINITY);
    }

    // LNS scores only placements it has already removed, so the occupancy never
    // contains this placement's own marks and there is nothing to discount. An
    // earlier revision cloned the whole occupancy here to subtract them, which
    // would have been a full four-bitset copy per candidate move.
    debug_assert!(
        solution.get(mv.placement).is_none(),
        "score_batch expects the placement to be unplaced; ruin removes it first"
    );

    let candidate = Occupant::of_offering(offering)
        .with_room(mv.to.room)
        .with_offering(problem.placement(mv.placement).offering);
    if !state.is_free(problem, &candidate, &span) {
        return Score(f64::INFINITY);
    }

    // MaxOnlineShare is NOT a feasibility filter (see `crate::aggregates`), so
    // its marginal effect is folded into the score instead: a candidate that
    // would push a (group, window) cell over its allowance is heavily
    // penalised, but remains reachable.
    let share_penalty = if state.would_worsen_share(problem, &candidate, &span) {
        problem.hard_penalty
    } else {
        0.0
    };

    /*
     * OnlineOnsiteSameDay is soft, so it is priced here at its CONFIGURED
     * WEIGHT rather than at `hard_penalty` like the share cap above. That
     * difference is the whole reclassification: a mix is now something the
     * search pays for and will accept when the alternative costs more, instead
     * of a candidate `is_free` threw away.
     *
     * Charged once for the move even when it would mix several cells. The exact
     * cell delta is what the counters report after `mark`, and the objective
     * reads it from there — this is a ranking signal for choosing between
     * candidates, and it only has to point the right way.
     */
    let day_mix_penalty = if state.would_worsen_day_mix(problem, &candidate, &span) {
        problem.day_mix_weight
    } else {
        0.0
    };

    // Compactness is soft, like day-mix, and its delta CAN be negative — a
    // candidate that fills a gap between two existing Sessions is rewarded,
    // not merely charged nothing. `compactness_delta` is a ranking signal, not
    // the exact per-placement charge; `mark` maintains the real one in
    // `Objective::compactness_cost` once this candidate is actually chosen.
    let compactness_delta = state.compactness_delta(problem, &candidate, &span);

    // Same ranking-signal contract as `compactness_delta` — see its own
    // comment above — for the Offering-scoped counterpart.
    let scheduling_pattern_delta = state.scheduling_pattern_delta(problem, &candidate, &span);

    Score(
        problem
            .soft
            .cost(offering.soft_profile, mv.to.start, mv.to.room)
            // EXACT, not a ranking approximation like the two penalties above:
            // a preference cost depends only on this placement and this slot,
            // so the delta the objective will record is the number read here.
            // The movement cost is exact for the same reason: it depends only
            // on this placement's `original` and the candidate, nothing else
            // already placed.
            + problem.preferences.cost(
                mv.placement,
                mv.to.start,
                &problem.rooms[mv.to.room.get()].features,
            )
            + problem.movement_cost(mv.placement, mv.to.start, mv.to.room)
            + share_penalty
            + day_mix_penalty
            + compactness_delta
            + scheduling_pattern_delta,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RoomIdx;
    use crate::testing;

    #[test]
    fn batch_scoring_flags_occupied_targets() {
        // 1 offering x 1 session, 2 rooms, 1 slot; room 0 already taken.
        let problem = testing::tiny_problem();
        let solution = Solution::empty(&problem);
        let mut occ = SearchState::from_fixed(&problem);
        let slot = problem.slots.resolve(0, 1, 0).unwrap();

        let blocker = Occupant::of_offering(&problem.offerings[0]).with_room(RoomIdx(0));
        occ.mark(&problem, &blocker, &[slot]);

        let moves = vec![
            Move { placement: PlacementIdx(0), to: Placement { start: slot, room: RoomIdx(0) } },
            Move { placement: PlacementIdx(0), to: Placement { start: slot, room: RoomIdx(1) } },
        ];
        let mut out = vec![Score::default(); moves.len()];

        CpuEvaluator.score_batch(&problem, &solution, &occ, &moves, &mut out);

        assert!(!out[0].0.is_finite(), "occupied room must be infeasible");
        assert_eq!(out[1], Score(0.0), "free room, no soft constraints, costs 0");
    }
}
