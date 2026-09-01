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

/// Whether `mv` could be placed right now — the same gates `score_one`
/// applies before costing, without the cost.
///
/// Exists for repair's exhaustive fallback (ADR-0031): when candidate
/// *sampling* finds nothing feasible for an unplaced Session, the question
/// becomes "does ANY feasible cell exist", and answering it with `score_batch`
/// would pay the full soft costing for cells that only need the bit tests.
pub fn is_feasible(problem: &Problem, state: &SearchState, mv: &Move) -> bool {
    let offering = problem.offering_of(mv.placement);

    let Some(span) = problem.slots.span(mv.to.start, offering.duration_blocks) else {
        return false;
    };
    if !offering.is_room_choice_eligible(mv.to.room, mv.to.additional_rooms) {
        return false;
    }
    if !offering.is_lecturer_choice_eligible(mv.to.lecturers) {
        return false;
    }

    let candidate = Occupant::of_offering(offering)
        .with_room(mv.to.room)
        .with_additional_rooms(mv.to.additional_rooms)
        .with_pool_lecturers(mv.to.lecturers)
        .with_offering(problem.placement(mv.placement).offering);
    state.is_free(problem, &candidate, &span)
}

fn score_one(problem: &Problem, solution: &Solution, state: &SearchState, mv: &Move) -> Score {
    let offering = problem.offering_of(mv.placement);

    let Some(span) = problem.slots.span(mv.to.start, offering.duration_blocks) else {
        // Would spill past the end of its day: not representable.
        return Score(f64::INFINITY);
    };
    if !offering.is_room_choice_eligible(mv.to.room, mv.to.additional_rooms) {
        return Score(f64::INFINITY);
    }
    if !offering.is_lecturer_choice_eligible(mv.to.lecturers) {
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
        .with_additional_rooms(mv.to.additional_rooms)
        .with_pool_lecturers(mv.to.lecturers)
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

    // Same ranking-signal contract as `share_penalty` — HARD, priced at
    // `hard_penalty` rather than a construction filter (ADR-0025), so a
    // candidate that would newly violate a day cap is heavily penalised but
    // remains reachable.
    let max_days_penalty = if state.would_worsen_max_days(problem, &candidate, &span) {
        problem.hard_penalty
    } else {
        0.0
    };
    let max_consecutive_days_penalty =
        if state.would_worsen_max_consecutive_days(problem, &candidate, &span) {
            problem.hard_penalty
        } else {
            0.0
        };

    // SOFT, unlike the two HARD penalties above: priced at the type's own
    // weight (whichever axis actually enforces it), the same
    // ranking-signal contract `day_mix_penalty` uses — a boolean worsening
    // check, not an exact delta.
    let daybreak_penalty = if state.would_worsen_daybreak(problem, &candidate, &span) {
        let mut w = 0.0;
        if candidate.enforce.daybreak_group {
            w += problem.daybreak_group_weight;
        }
        if candidate.enforce.daybreak_person {
            w += problem.daybreak_person_weight;
        }
        w
    } else {
        0.0
    };

    // Same ranking-signal contract as `daybreak_penalty`.
    let travel_penalty = if state.would_worsen_travel(problem, &candidate, &span) {
        let mut w = 0.0;
        if candidate.enforce.travel_group {
            w += problem.travel_group_weight;
        }
        if candidate.enforce.travel_person {
            w += problem.travel_person_weight;
        }
        w
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

    // Same ranking-signal contract as `day_mix_penalty`, one per exam
    // spacing type.
    let exam_same_day_penalty = if state.would_worsen_exam_same_day(problem, &candidate, &span) {
        problem.exam_same_day_weight
    } else {
        0.0
    };
    let exam_window_penalty = if state.would_worsen_exam_window(problem, &candidate, &span) {
        problem.exam_window_weight
    } else {
        0.0
    };

    // Same ranking-signal contract as `max_daily_span_delta` — over
    // per-week weekday-count variance instead of a first-to-last span.
    let imbalance_delta = state.imbalance_delta(problem, &candidate, &span);

    // Compactness is soft, like day-mix, and its delta CAN be negative — a
    // candidate that fills a gap between two existing Sessions is rewarded,
    // not merely charged nothing. `compactness_delta` is a ranking signal, not
    // the exact per-placement charge; `mark` maintains the real one in
    // `Objective::compactness_cost` once this candidate is actually chosen.
    let compactness_delta = state.compactness_delta(problem, &candidate, &span);

    // Same ranking-signal contract as `compactness_delta` — the mirror
    // image, over run-excess instead of gap count.
    let max_consecutive_delta = state.max_consecutive_delta(problem, &candidate, &span);

    // Same ranking-signal contract again, over span-excess.
    let max_daily_span_delta = state.max_daily_span_delta(problem, &candidate, &span);

    // Same ranking-signal contract as `max_daily_span_delta` — a raw
    // count-excess instead of span-excess.
    let max_daily_session_count_delta =
        state.max_daily_session_count_delta(problem, &candidate, &span);

    // Same ranking-signal contract, keyed by lecturer and week rather than
    // by Group/Person and day.
    let max_weekly_teaching_load_delta =
        state.max_weekly_teaching_load_delta(problem, &candidate, &span);

    // Same ranking-signal contract as `max_daily_span_delta` — over
    // distinct-location excess instead of span-excess.
    let location_change_delta = state.location_change_delta(problem, &candidate, &span);

    // Same ranking-signal contract again — over Room-adjacency boundary
    // violations instead of a Group/Person axis.
    let room_turnaround_delta = state.room_turnaround_delta(problem, &candidate, &span);

    // Same ranking-signal contract as `max_weekly_teaching_load_delta` — keyed
    // by Group and week, over distinct-Room excess instead of a headcount.
    let room_churn_delta = state.room_churn_delta(problem, &candidate, &span);

    // Same ranking-signal contract as `scheduling_pattern_delta` — keyed by
    // Offering with no day/week axis, over modal-Room excess.
    let room_consistency_delta = state.room_consistency_delta(problem, &candidate, &span);

    // Same ranking-signal contract as `room_consistency_delta`/
    // `max_daily_session_count_delta` — the (offering, day) cluster:
    // count, longest-run and split-run excess, keyed by Offering.
    let offering_daily_count_delta = state.offering_daily_count_delta(problem, &candidate, &span);
    let offering_run_delta = state.offering_run_delta(problem, &candidate, &span);
    let offering_split_delta = state.offering_split_delta(problem, &candidate, &span);

    // Same ranking-signal contract as `room_consistency_delta` — over
    // distinct-lecturer excess instead of modal-Room excess, inert for any
    // Offering without a genuine lecturer pool.
    let lecturer_consistency_delta = state.lecturer_consistency_delta(problem, &candidate, &span);

    // Same ranking-signal contract as `compactness_delta` — see its own
    // comment above — for the Offering-scoped counterpart.
    let scheduling_pattern_delta = state.scheduling_pattern_delta(problem, &candidate, &span);

    // Same ranking-signal contract, over an Offering's distinct-day count
    // instead of its modal pattern — independent axis, same `who.offering`
    // gate.
    let offering_distinct_days_delta =
        state.offering_distinct_days_delta(problem, &candidate, &span);

    let capacity = problem.exclusive_capacity(mv.to.all_rooms());

    Score(
        mv.to
            .all_rooms()
            .map(|r| problem.soft.cost(offering.soft_profile, mv.to.start, r))
            .sum::<f64>()
            // EXACT, not a ranking approximation like the two penalties above:
            // a preference cost depends only on this placement and this slot,
            // so the delta the objective will record is the number read here.
            // The movement cost is exact for the same reason: it depends only
            // on this placement's `original` and the candidate, nothing else
            // already placed. The capacity-waste cost is exact too — it
            // depends only on this Offering's `min_capacity` and the
            // candidate's own Room set.
            + problem.preference_cost_for_placement(offering, mv.placement, mv.to)
            + problem.movement_cost(mv.placement, mv.to.start, mv.to.room)
            + problem.capacity_waste_cost(offering, capacity)
            + problem.specialized_room_cost(offering, mv.to.all_rooms())
            + problem.break_spanning_cost(offering, mv.to.start, offering.duration_blocks)
            + share_penalty
            + max_days_penalty
            + max_consecutive_days_penalty
            + daybreak_penalty
            + travel_penalty
            + day_mix_penalty
            + exam_same_day_penalty
            + exam_window_penalty
            + compactness_delta
            + max_consecutive_delta
            + max_daily_span_delta
            + max_daily_session_count_delta
            + max_weekly_teaching_load_delta
            + imbalance_delta
            + location_change_delta
            + room_turnaround_delta
            + room_churn_delta
            + room_consistency_delta
            + lecturer_consistency_delta
            + offering_daily_count_delta
            + offering_run_delta
            + offering_split_delta
            + scheduling_pattern_delta
            + offering_distinct_days_delta,
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
            Move { placement: PlacementIdx(0), to: Placement::single(slot, RoomIdx(0)) },
            Move { placement: PlacementIdx(0), to: Placement::single(slot, RoomIdx(1)) },
        ];
        let mut out = vec![Score::default(); moves.len()];

        CpuEvaluator.score_batch(&problem, &solution, &occ, &moves, &mut out);

        assert!(!out[0].0.is_finite(), "occupied room must be infeasible");
        assert_eq!(out[1], Score(0.0), "free room, no soft constraints, costs 0");

        // The cheap gate must agree with the priced one, cell for cell —
        // repair's exhaustive fallback (ADR-0031) substitutes it for scoring.
        assert!(!is_feasible(&problem, &occ, &moves[0]));
        assert!(is_feasible(&problem, &occ, &moves[1]));
    }
}
