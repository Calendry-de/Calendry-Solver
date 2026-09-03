//! The from-scratch objective: full recomputation, the per-instance
//! breakdown, and the drift check the incremental path is held against.

use crate::constraints;
use crate::problem::Problem;
use crate::soft::{Objective, RankSpan, SoftComponent};
use crate::solution::{SearchState, Solution};

/// Full recomputation, in placement-index order so the `f64` fold is
/// bit-reproducible.
pub fn recompute_objective(problem: &Problem, solution: &Solution) -> Objective {
    let mut unplaced = 0u32;
    let mut soft = 0.0f64;
    for p in problem.placement_ids() {
        match solution.get(p) {
            Some(pl) => {
                let o = problem.offering_of(p);
                let capacity = problem.exclusive_capacity(pl.all_rooms());
                soft += pl
                    .all_rooms()
                    .map(|r| problem.soft.cost(o.soft_profile, pl.start, r))
                    .sum::<f64>()
                    + problem
                        .preferences
                        .cost(p, pl.start, &problem.rooms[pl.room.get()].features)
                    + problem.movement_cost(p, pl.start, pl.room)
                    + problem.capacity_waste_cost(o, capacity)
                    + problem.specialized_room_cost(o, pl.all_rooms())
                    + problem.break_spanning_cost(o, pl.start, o.duration_blocks)
                    + problem.exam_week_cost(o, pl.start);
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
        max_days_violations: state.max_days_violations(),
        max_consecutive_days_violations: state.max_consecutive_days_violations(),
        same_time_violations: constraints::same_time_violations(problem, solution),
        same_days_violations: constraints::same_days_violations(problem, solution),
        same_start_violations: constraints::same_start_violations(problem, solution),
        precedence_violations: constraints::precedence_violations(problem, solution),
        soft,
        day_mix_cost: state.day_mix_cost(problem),
        compactness_cost: state.compactness_cost(problem),
        max_consecutive_cost: state.max_consecutive_cost(problem),
        max_daily_span_cost: state.max_daily_span_cost(problem),
        max_daily_session_count_cost: state.max_daily_session_count_cost(problem),
        max_weekly_teaching_load_cost: state.max_weekly_teaching_load_cost(problem),
        exam_same_day_cost: state.exam_same_day_cost(problem),
        exam_window_cost: state.exam_window_cost(problem),
        imbalance_cost: state.imbalance_cost(problem),
        location_change_cost: state.location_change_cost(problem),
        room_turnaround_cost: state.room_turnaround_cost(problem),
        room_churn_cost: state.room_churn_cost(problem),
        room_consistency_cost: state.room_consistency_cost(problem),
        lecturer_consistency_cost: state.lecturer_consistency_cost(problem),
        offering_daily_count_cost: state.offering_daily_count_cost(problem),
        offering_run_cost: state.offering_run_cost(problem),
        offering_split_cost: state.offering_split_cost(problem),
        scheduling_pattern_cost: state.scheduling_pattern_cost(problem),
        daybreak_cost: state.daybreak_cost(problem),
        travel_cost: state.travel_cost(problem),
        offering_distinct_days_cost: state.offering_distinct_days_cost(problem),
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

    /*
     * The preference instances come separately for the same reason the day-mix
     * ones do — they are not in `problem.soft` — but unlike day-mix their cost
     * IS already inside `Objective::soft`. What this loop rebuilds is the
     * per-instance attribution, which the accumulated total cannot supply once
     * two instances with different kind scopes have been summed into one number.
     *
     * `raw_count` is "placed Sessions that missed something a lecturer asked
     * for", the question a person actually asks, and `weighted` is exactly what
     * the objective charged for them.
     */
    let preference = problem
        .constraints
        .person_preference_fit
        .iter()
        .map(|inst| {
            let mut count = 0u64;
            let mut weighted = 0.0f64;

            for p in problem.placement_ids() {
                let Some(pl) = solution.get(p) else { continue };
                if !inst.covers(&problem.offering_of(p).kind) {
                    continue;
                }
                // The UNMET fraction, so a Session whose lecturers got exactly what
                // they asked for reports nothing rather than reporting a success.
                let unmet =
                    problem
                        .preferences
                        .unmet(p, pl.start, &problem.rooms[pl.room.get()].features);
                if unmet > 0.0 {
                    count += 1;
                }
                weighted += inst.weight * unmet;
            }

            SoftComponent {
                constraint_id: inst.id.clone(),
                constraint_type: constraints::ConstraintType::PersonPreferenceFit.as_str(),
                raw_count: count,
                weighted,
            }
        });

    /*
     * And the exam-week instances, for the third time the same reason: since
     * ADR-0033 an exam week may be scoped to Groups, so the type left
     * `problem.soft` and its cost comes from `Problem::exam_week_cost`
     * instead. Without this block the type would vanish from the breakdown
     * while still moving the score, which ADR-0024 names as the failure to
     * avoid — the breakdown is what the app shows a human to explain it.
     *
     * `raw_count` is charged placements and `weighted` is exactly what the
     * objective charged, read through the same per-Offering mask the search
     * used rather than through a second reading of the calendar.
     */
    let exam_week = problem.constraints.minimize_exam_week.iter().map(|inst| {
        let mut count = 0u64;
        let mut weighted = 0.0f64;

        for p in problem.placement_ids() {
            let Some(pl) = solution.get(p) else { continue };
            let o = problem.offering_of(p);
            if !inst.covers(&o.kind) {
                continue;
            }
            // The same side-of-the-mask question `exam_week_cost` asks, per
            // instance rather than per Offering, so two instances with
            // different kind scopes stay separately attributable.
            if o.exam_week_slots.contains(pl.start.get()) != inst.invert {
                count += 1;
                weighted += inst.weight;
            }
        }

        SoftComponent {
            constraint_id: inst.id.clone(),
            constraint_type: constraints::ConstraintType::MinimizeExamWeek.as_str(),
            raw_count: count,
            weighted,
        }
    });

    day_mix
        .chain(preference)
        .chain(exam_week)
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
        && a.max_days_violations == b.max_days_violations
        && a.max_consecutive_days_violations == b.max_consecutive_days_violations
        && a.same_time_violations == b.same_time_violations
        && a.same_days_violations == b.same_days_violations
        && a.same_start_violations == b.same_start_violations
        && a.precedence_violations == b.precedence_violations
        && (a.soft - b.soft).abs() <= 1e-9 * (1.0 + a.soft.abs())
        && (a.day_mix_cost - b.day_mix_cost).abs() <= 1e-9 * (1.0 + a.day_mix_cost.abs())
        && (a.compactness_cost - b.compactness_cost).abs()
            <= 1e-9 * (1.0 + a.compactness_cost.abs())
        && (a.max_consecutive_cost - b.max_consecutive_cost).abs()
            <= 1e-9 * (1.0 + a.max_consecutive_cost.abs())
        && (a.max_daily_span_cost - b.max_daily_span_cost).abs()
            <= 1e-9 * (1.0 + a.max_daily_span_cost.abs())
        && (a.max_daily_session_count_cost - b.max_daily_session_count_cost).abs()
            <= 1e-9 * (1.0 + a.max_daily_session_count_cost.abs())
        && (a.max_weekly_teaching_load_cost - b.max_weekly_teaching_load_cost).abs()
            <= 1e-9 * (1.0 + a.max_weekly_teaching_load_cost.abs())
        && (a.exam_same_day_cost - b.exam_same_day_cost).abs()
            <= 1e-9 * (1.0 + a.exam_same_day_cost.abs())
        && (a.exam_window_cost - b.exam_window_cost).abs()
            <= 1e-9 * (1.0 + a.exam_window_cost.abs())
        && (a.imbalance_cost - b.imbalance_cost).abs() <= 1e-9 * (1.0 + a.imbalance_cost.abs())
        && (a.location_change_cost - b.location_change_cost).abs()
            <= 1e-9 * (1.0 + a.location_change_cost.abs())
        && (a.room_turnaround_cost - b.room_turnaround_cost).abs()
            <= 1e-9 * (1.0 + a.room_turnaround_cost.abs())
        && (a.daybreak_cost - b.daybreak_cost).abs() <= 1e-9 * (1.0 + a.daybreak_cost.abs())
        && (a.travel_cost - b.travel_cost).abs() <= 1e-9 * (1.0 + a.travel_cost.abs())
        && (a.offering_distinct_days_cost - b.offering_distinct_days_cost).abs()
            <= 1e-9 * (1.0 + a.offering_distinct_days_cost.abs())
        && (a.room_churn_cost - b.room_churn_cost).abs() <= 1e-9 * (1.0 + a.room_churn_cost.abs())
        && (a.room_consistency_cost - b.room_consistency_cost).abs()
            <= 1e-9 * (1.0 + a.room_consistency_cost.abs())
        && (a.lecturer_consistency_cost - b.lecturer_consistency_cost).abs()
            <= 1e-9 * (1.0 + a.lecturer_consistency_cost.abs())
        && (a.offering_daily_count_cost - b.offering_daily_count_cost).abs()
            <= 1e-9 * (1.0 + a.offering_daily_count_cost.abs())
        && (a.offering_run_cost - b.offering_run_cost).abs()
            <= 1e-9 * (1.0 + a.offering_run_cost.abs())
        && (a.offering_split_cost - b.offering_split_cost).abs()
            <= 1e-9 * (1.0 + a.offering_split_cost.abs())
        && (a.scheduling_pattern_cost - b.scheduling_pattern_cost).abs()
            <= 1e-9 * (1.0 + a.scheduling_pattern_cost.abs())
}
