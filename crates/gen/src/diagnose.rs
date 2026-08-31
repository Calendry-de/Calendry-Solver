//! Why construction fails: attribute every rejected `(slot, room)` to an axis.
//!
//! Slice 5 measured that construction is 93% of a large-university run and leaves
//! 2,468 of 25,520 Sessions unplaced. That is *what* happens. This module is for
//! *why*, because the fix depends entirely on the shape of the failure:
//!
//! * If failed placements have **zero** free `(slot, room)` pairs in the final
//!   state, the greedy ordering painted itself into a corner and the answer is
//!   about ordering or backtracking.
//! * If they have **some** free pairs, construction's first-fit scan should have
//!   found them — so the failure is in *when* it looked, not whether space
//!   exists, and LNS repair should be able to recover them.
//! * Which axis does the blocking — room, group, lecturer, person, veto, day-mix
//!   — decides whether the answer is more rooms, better group packing, or
//!   something else entirely.
//!
//! Attribution works by exploiting the fact that [`Occupant::enforce`] is public:
//! a probe with a single axis enabled isolates that axis exactly, using the same
//! `is_free` the search uses. No parallel reimplementation of the rule, and
//! nothing added to core for the sake of measurement.

use calendry_solver_core::ids::PlacementIdx;
use calendry_solver_core::problem::{Enforce, Problem};
use calendry_solver_core::solution::{Occupant, SearchState, Solution};

/// Enables exactly one axis on an [`Enforce`], isolating it for attribution.
type AxisProbe = fn(&mut Enforce);

/// Per-axis rejection counts for one unplaced placement.
#[derive(Clone, Debug, Default)]
pub struct Attribution {
    pub candidates: u64,
    /// Candidates that pass **every** check — space the placement could occupy
    /// right now.
    pub free: u64,
    pub blocked_room: u64,
    pub blocked_lecturer: u64,
    pub blocked_group: u64,
    pub blocked_person: u64,
    pub blocked_veto: u64,
    /// Candidates the day-mix rule would CHARGE for rather than reject.
    /// Replaced `blocked_day_mix` when the rule became soft.
    pub day_mix_priced: u64,
}

/// Aggregate over the sampled unplaced placements.
#[derive(Clone, Debug, Default)]
pub struct ConstructionFailure {
    pub sampled: usize,
    pub total_unplaced: usize,
    pub totals: Attribution,
    /// How many sampled placements had at least one free `(slot, room)`.
    pub with_free_space: usize,
    /// Free-space histogram, bucketed: 0, 1-9, 10-99, 100-999, 1000+.
    pub free_buckets: [usize; 5],
    /// Free candidates as a fraction of the candidate space, for placements
    /// that have any. Drives the sampling-hit-rate question.
    pub min_free_ratio: f64,
    pub median_free_ratio: f64,
    /// Unplaced placements by Offering kind.
    pub by_kind: Vec<(String, usize)>,
    pub mean_eligible_rooms: f64,
    /// Pairwise person-axis conflict density among Offerings of the dominant
    /// unplaced kind, and what it implies for feasibility.
    pub clique: Option<CliqueEvidence>,
    /// Whether the sample came from unplaced placements or, when construction
    /// succeeded, from placed ones.
    pub pool_was_unplaced: bool,
    /// Start slots examined across the sample.
    pub slots_examined: u64,
    /// Of those, slots rejected by a **room-independent** axis — lecturer,
    /// group, person or veto. At such a slot `construct`'s inner room loop is
    /// pure waste: it re-tests the same room-independent state once per eligible
    /// Room and can never succeed.
    pub slots_blocked_room_independent: u64,
    /// Candidate probes that hoisting the room-independent checks out of the
    /// room loop would avoid.
    pub wasted_probes: u64,
}

/// Evidence that a set of Sessions is **mutually exclusive in time**.
///
/// Per-entity load metrics — group row, lecturer row, room tightness — cannot
/// see this. Each individual is lightly loaded; what makes the set unplaceable
/// is that its members pairwise share an attendee, so no two can ever share a
/// slot no matter how light each one is.
#[derive(Clone, Debug)]
pub struct CliqueEvidence {
    pub kind: String,
    pub pairs_sampled: usize,
    /// Fraction of sampled pairs sharing at least one attendee.
    pub conflict_density: f64,
    /// Sessions of this kind that must be placed.
    pub sessions: u64,
    /// Non-overlapping slots available for them, at this kind's duration.
    pub capacity: u64,
}

fn single(axis: fn(&mut Enforce), base: Enforce) -> Enforce {
    // Only probe axes the problem actually enforces for this kind: a disabled
    // check can never be the reason a placement failed, and counting it would
    // invent a cause.
    let mut e = Enforce::default();
    let mut probe = Enforce {
        room: true,
        lecturer: true,
        group: true,
        person: true,
        lecturer_veto: true,
        group_veto: true,
        day_mix: true,
        // Never probed: neither gates `is_free`, same as `day_mix` above
        // being probed anyway is about attributing SOFT badness rather than
        // rejection — Compactness has no `would_worsen`-style predicate this
        // tool could probe against yet, so it stays false rather than
        // guessing at one.
        compactness_group: false,
        compactness_person: false,
        // Same reasoning as Compactness above: neither pattern type has a
        // `would_worsen`-style predicate to probe.
        distributed_pattern: false,
        block_pattern: false,
        // Never probed either: filterable, but not yet added to the `axes`
        // list below — nothing has needed a `ProtectedBlock`-attributed
        // rejection count yet.
        protected_block: false,
        // Same reasoning as Compactness above: no `would_worsen`-style
        // predicate to probe.
        max_consecutive_group: false,
        max_consecutive_person: false,
        max_daily_span_group: false,
        max_daily_span_person: false,
        max_daily_session_count_group: false,
        max_daily_session_count_person: false,
        max_consecutive_offering_blocks: false,
        max_offering_sessions_per_day: false,
        minimize_offering_day_split: false,
        max_weekly_teaching_load: false,
        exam_spacing_same_day: false,
        exam_spacing_window: false,
        minimize_weekday_imbalance: false,
        minimize_location_change_group: false,
        minimize_location_change_person: false,
        room_turnaround: false,
        minimize_room_churn: false,
        room_consistency: false,
        lecturer_consistency: false,
        // Same reasoning as Compactness above: no `would_worsen`-style
        // predicate wired into this tool yet, even though one exists on
        // `Aggregates` for the search's own ranking use.
        max_days_group: false,
        max_days_person: false,
        max_consecutive_days_group: false,
        max_consecutive_days_person: false,
        daybreak_group: false,
        daybreak_person: false,
    };
    axis(&mut e);
    axis(&mut probe);
    Enforce {
        room: e.room && base.room,
        lecturer: e.lecturer && base.lecturer,
        group: e.group && base.group,
        person: e.person && base.person,
        lecturer_veto: e.lecturer_veto && base.lecturer_veto,
        group_veto: e.group_veto && base.group_veto,
        day_mix: e.day_mix && base.day_mix,
        compactness_group: e.compactness_group && base.compactness_group,
        compactness_person: e.compactness_person && base.compactness_person,
        distributed_pattern: e.distributed_pattern && base.distributed_pattern,
        block_pattern: e.block_pattern && base.block_pattern,
        protected_block: e.protected_block && base.protected_block,
        max_consecutive_group: e.max_consecutive_group && base.max_consecutive_group,
        max_consecutive_person: e.max_consecutive_person && base.max_consecutive_person,
        max_daily_span_group: e.max_daily_span_group && base.max_daily_span_group,
        max_daily_span_person: e.max_daily_span_person && base.max_daily_span_person,
        max_daily_session_count_group: e.max_daily_session_count_group
            && base.max_daily_session_count_group,
        max_daily_session_count_person: e.max_daily_session_count_person
            && base.max_daily_session_count_person,
        max_consecutive_offering_blocks: e.max_consecutive_offering_blocks
            && base.max_consecutive_offering_blocks,
        max_offering_sessions_per_day: e.max_offering_sessions_per_day
            && base.max_offering_sessions_per_day,
        minimize_offering_day_split: e.minimize_offering_day_split
            && base.minimize_offering_day_split,
        max_weekly_teaching_load: e.max_weekly_teaching_load && base.max_weekly_teaching_load,
        exam_spacing_same_day: e.exam_spacing_same_day && base.exam_spacing_same_day,
        exam_spacing_window: e.exam_spacing_window && base.exam_spacing_window,
        minimize_weekday_imbalance: e.minimize_weekday_imbalance && base.minimize_weekday_imbalance,
        minimize_location_change_group: e.minimize_location_change_group
            && base.minimize_location_change_group,
        minimize_location_change_person: e.minimize_location_change_person
            && base.minimize_location_change_person,
        room_turnaround: e.room_turnaround && base.room_turnaround,
        minimize_room_churn: e.minimize_room_churn && base.minimize_room_churn,
        room_consistency: e.room_consistency && base.room_consistency,
        lecturer_consistency: e.lecturer_consistency && base.lecturer_consistency,
        max_days_group: e.max_days_group && base.max_days_group,
        max_days_person: e.max_days_person && base.max_days_person,
        max_consecutive_days_group: e.max_consecutive_days_group && base.max_consecutive_days_group,
        max_consecutive_days_person: e.max_consecutive_days_person
            && base.max_consecutive_days_person,
        daybreak_group: e.daybreak_group && base.daybreak_group,
        daybreak_person: e.daybreak_person && base.daybreak_person,
    }
}

/// Scan the whole candidate space of each unplaced placement and record which
/// axis rejects each `(slot, room)`.
///
/// `limit` caps how many unplaced placements are examined — the full scan is
/// `unplaced x starts x rooms x axes`, which is far too much at university
/// scale.
pub fn diagnose(
    problem: &Problem,
    solution: &Solution,
    state: &SearchState,
    limit: usize,
) -> ConstructionFailure {
    let unplaced: Vec<PlacementIdx> = problem
        .placement_ids()
        .filter(|&p| solution.get(p).is_none())
        .collect();

    let mut out = ConstructionFailure {
        total_unplaced: unplaced.len(),
        pool_was_unplaced: !unplaced.is_empty(),
        ..Default::default()
    };

    // When construction succeeded there is no failure to explain, but the
    // *scan cost* question is still open and is answered on placed placements:
    // how much of first-fit's inner room loop was wasted on slots that a
    // room-independent axis had already ruled out.
    let pool: Vec<PlacementIdx> =
        if unplaced.is_empty() { problem.placement_ids().collect() } else { unplaced };
    if pool.is_empty() {
        return out;
    }

    // Even stride, so the sample spans the whole run of construction rather
    // than only the placements it gave up on first.
    let stride = (pool.len() / limit.max(1)).max(1);
    let sample: Vec<PlacementIdx> = pool.iter().copied().step_by(stride).collect();

    /*
     * FIVE AXES, NOT SIX. `day_mix` was the sixth until OnlineOnsiteSameDay
     * became soft, and it is gone from here for a reason worth stating: this
     * probe asks `is_free`, so a soft rule can only ever answer "blocks
     * nothing". Leaving it in would print `day_mix 0.00%` on every instance —
     * true, and read by anybody as "this rule never binds" rather than "this
     * rule no longer filters".
     *
     * What replaces it is `day_mix_priced` below, which counts the candidates
     * the rule would CHARGE for. That is the same question the old line
     * answered, asked in the terms the rule now works in.
     */
    let axes: [AxisProbe; 5] = [
        |e| e.room = true,
        |e| e.lecturer = true,
        |e| e.group = true,
        |e| e.person = true,
        |e| e.lecturer_veto = true,
    ];

    let mut ratios: Vec<f64> = Vec::new();
    let mut kinds: Vec<(String, usize)> = Vec::new();
    let mut rooms_sum = 0u64;

    for &p in &sample {
        let offering = problem.offering_of(p);
        let base = Occupant::of_offering(offering);
        let enforce = offering.enforce;
        let mut a = Attribution::default();
        rooms_sum += offering.eligible_rooms.len() as u64;

        match kinds.iter_mut().find(|(k, _)| *k == offering.kind) {
            Some((_, n)) => *n += 1,
            None => kinds.push((offering.kind.clone(), 1)),
        }

        let n_starts = problem.slots.start_count(offering.duration_blocks);
        for i in 0..n_starts {
            let start = problem
                .slots
                .nth_start(offering.duration_blocks, i)
                .expect("index below start_count");
            // One allocation per start slot rather than per candidate.
            let Some(span) = problem.slots.span(start, offering.duration_blocks) else {
                continue;
            };

            // Room-independent axes, tested ONCE for this slot. If they reject,
            // every probe the room loop is about to make is wasted.
            out.slots_examined += 1;
            // The SAME mask the heuristic uses, from the one place that defines
            // it. This used to be a verbatim copy of `construct`'s literal, and
            // the whole point of this function is reporting where construction
            // rejects candidates — which it can only do truthfully if its filter
            // order matches. A seventh axis would have left it reporting against
            // the old mask, silently, with plausible numbers. (The copy also
            // indexed `eligible_rooms[0]`, which panics on an Offering with no
            // eligible room; the shared constructor sets no room at all, because
            // the room axis is exactly what this probe excludes.)
            if let Some(ri) = Occupant::room_independent_probe(offering)
                && !state.is_free(problem, &ri, &span)
            {
                out.slots_blocked_room_independent += 1;
                out.wasted_probes += offering.eligible_rooms.len() as u64;
            }

            for &room in &offering.eligible_rooms {
                a.candidates += 1;
                let candidate = base.with_room(room);
                // Priced, not blocked: a candidate that would mix a day is
                // still free, and this counts how often the rule has something
                // to say about the search's choices.
                if state.would_worsen_day_mix(problem, &candidate, &span) {
                    a.day_mix_priced += 1;
                }

                if state.is_free(problem, &candidate, &span) {
                    a.free += 1;
                    continue;
                }
                // Blocked — find every axis that would reject it on its own.
                // Not "the first one": knowing a candidate is blocked by three
                // axes at once is the difference between "free a room" and
                // "nothing here will help".
                for (n, axis) in axes.iter().enumerate() {
                    // Per-axis attribution: "would THIS axis reject it alone".
                    // Genuinely diagnostic-only, so it keeps its own mask — but
                    // it goes through the named builder rather than assigning a
                    // public field from outside.
                    let probe = candidate.with_enforce(single(*axis, enforce));
                    if probe.enforce == Enforce::default() {
                        continue; // this axis is switched off for this kind
                    }
                    if !state.is_free(problem, &probe, &span) {
                        match n {
                            0 => a.blocked_room += 1,
                            1 => a.blocked_lecturer += 1,
                            2 => a.blocked_group += 1,
                            _ => a.blocked_veto += 1,
                        }
                    }
                }
            }
        }

        let bucket = match a.free {
            0 => 0,
            1..=9 => 1,
            10..=99 => 2,
            100..=999 => 3,
            _ => 4,
        };
        out.free_buckets[bucket] += 1;
        if a.free > 0 {
            out.with_free_space += 1;
            ratios.push(a.free as f64 / a.candidates.max(1) as f64);
        }

        out.totals.candidates += a.candidates;
        out.totals.free += a.free;
        out.totals.blocked_room += a.blocked_room;
        out.totals.blocked_lecturer += a.blocked_lecturer;
        out.totals.blocked_group += a.blocked_group;
        out.totals.blocked_person += a.blocked_person;
        out.totals.blocked_veto += a.blocked_veto;
        out.totals.day_mix_priced += a.day_mix_priced;
    }

    // If one kind dominates the failures, ask whether that kind's Sessions can
    // coexist at all.
    if let Some((kind, _)) = kinds.iter().max_by_key(|(_, n)| *n) {
        out.clique = Some(clique_evidence(problem, kind));
    }

    ratios.sort_by(f64::total_cmp);
    out.sampled = sample.len();
    out.min_free_ratio = ratios.first().copied().unwrap_or(0.0);
    out.median_free_ratio = ratios.get(ratios.len() / 2).copied().unwrap_or(0.0);
    kinds.sort_by_key(|k| std::cmp::Reverse(k.1));
    out.by_kind = kinds;
    out.mean_eligible_rooms = rooms_sum as f64 / sample.len().max(1) as f64;
    out
}

/// Sample Offering pairs of one kind and ask how often they share an attendee.
///
/// Two Offerings sharing even one attendee can never occupy the same slot under
/// `PersonDoubleBooking`. If that is true of *most* pairs, the whole kind forms a
/// near-clique in the conflict graph, and its Sessions need one distinct slot
/// each — a hard capacity bound that no search or ordering can get around.
fn clique_evidence(problem: &Problem, kind: &str) -> CliqueEvidence {
    let of_kind: Vec<usize> = problem
        .offerings
        .iter()
        .enumerate()
        .filter(|(_, o)| o.kind == kind)
        .map(|(i, _)| i)
        .collect();

    // Deterministic stride-sampled pairs; enough to estimate a density without
    // paying O(n^2) on thousands of Offerings.
    let mut sampled = 0usize;
    let mut conflicting = 0usize;
    'outer: for (a_pos, &a) in of_kind.iter().enumerate() {
        for &b in of_kind.iter().skip(a_pos + 1).step_by(7) {
            let (x, y) = (&problem.offerings[a], &problem.offerings[b]);
            // Attendee lists are sorted and deduplicated by `Problem::build`.
            let shares = x
                .attendees
                .iter()
                .any(|p| y.attendees.binary_search(p).is_ok());
            sampled += 1;
            if shares {
                conflicting += 1;
            }
            if sampled >= 4000 {
                break 'outer;
            }
        }
    }

    let sessions: u64 = problem
        .placement_ids()
        .filter(|&p| problem.offering_of(p).kind == kind)
        .count() as u64;

    // How many of this kind could be placed even in an empty grid, if every one
    // of them conflicts with every other: one per non-overlapping slot.
    let duration = of_kind
        .first()
        .map_or(1, |&i| problem.offerings[i].duration_blocks)
        .max(1);
    let capacity = (problem.slots.len() as u64) / duration as u64;

    CliqueEvidence {
        kind: kind.to_string(),
        pairs_sampled: sampled,
        conflict_density: conflicting as f64 / sampled.max(1) as f64,
        sessions,
        capacity,
    }
}
