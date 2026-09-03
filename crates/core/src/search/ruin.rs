//! The ruin half of LNS: four operators choosing what one round releases,
//! and the repair order the round hands back (ADR-0031).

use crate::ids::PlacementIdx;
use crate::problem::{Offering, Problem};
use crate::rng::Rng;
use crate::solution::{Occupant, Placement, SearchState, Solution};

use super::repair::CandidateSpace;
use super::trial::Trial;
use super::tuning;

/// Remove a handful of placements, releasing their occupancy.
///
/// Four operators, chosen by the seeded RNG (the fourth, [`ruin_blocking`],
/// only while Sessions remain unplaced). `Related` is what lets the search
/// *swap* two Sessions: any one-at-a-time neighbourhood has to pass through an
/// infeasible intermediate to reach a swap, so without it those moves are
/// unreachable.
///
/// `cap` bounds how many PLACED Sessions one round may disturb — the base cap
/// at level 0, doubled per escalation level (ADR-0031).
///
/// Returns the removal set in **repair order** — previously-unplaced Sessions
/// first, then the ruined, each class shuffled with the seeded RNG — with each
/// entry flagged `true` if it was unplaced before this round. Unplaced-first is
/// aligned with the objective (placing one outranks any soft cost a re-placed
/// Session could recover); the shuffles matter because a fixed ascending order
/// hands every contested freed cell to the lowest index forever — the same
/// neighbourhood collapse `repair_one`'s tie-break fix records, one level up.
///
/// The removed positions no longer come back as a second return value: the
/// `Trial`'s journal records them, so the undo is its business rather than the
/// caller's.
pub(super) fn ruin(
    problem: &Problem,
    trial: &mut Trial<'_>,
    rng: &mut Rng,
    cap: usize,
) -> Vec<(PlacementIdx, bool)> {
    // Selection reads the solution; removal mutates the trial. Scoped so the
    // shared borrow ends before the exclusive one begins.
    let ordered = {
        let current = trial.solution();
        let placed: Vec<PlacementIdx> = problem
            .placement_ids()
            .filter(|&p| current.get(p).is_some())
            .collect();

        // Anything construction failed to place is retried on every iteration.
        // Without this, ruin only ever selects PLACED Sessions, so a Session
        // that greedy dead-ended on could never be reconsidered and the
        // `unplaced` term of the objective would be permanently unoptimizable.
        let mut unplaced: Vec<PlacementIdx> = problem
            .placement_ids()
            .filter(|&p| current.get(p).is_none())
            .collect();

        let mut ruined = if placed.is_empty() {
            // Nothing to release; the unplaced simply form the repair list.
            Vec::new()
        } else {
            // Ruin size: at least 1, at most `cap` or the number placed,
            // whichever is smaller.
            let max_k = placed.len().clamp(1, cap);
            let k = 1 + rng.below(max_k);

            let mut chosen = if unplaced.is_empty() {
                match rng.below(3) {
                    0 => ruin_random(&placed, k, rng),
                    1 => ruin_worst(problem, current, trial.state(), &placed, k),
                    _ => ruin_related(problem, current, &placed, k, rng),
                }
            } else {
                match rng.below(4) {
                    0 => ruin_random(&placed, k, rng),
                    1 => ruin_worst(problem, current, trial.state(), &placed, k),
                    2 => ruin_related(problem, current, &placed, k, rng),
                    _ => ruin_blocking(problem, current, &placed, &unplaced, max_k, rng),
                }
            };
            chosen.sort_unstable();
            chosen.dedup();
            chosen
        };

        rng.shuffle(&mut unplaced);
        rng.shuffle(&mut ruined);
        let mut ordered: Vec<(PlacementIdx, bool)> =
            Vec::with_capacity(unplaced.len() + ruined.len());
        ordered.extend(unplaced.into_iter().map(|p| (p, true)));
        ordered.extend(ruined.into_iter().map(|p| (p, false)));
        ordered
    };

    for &(p, was_unplaced) in &ordered {
        let released = trial.unplace(p);
        debug_assert_eq!(
            released.is_none(),
            was_unplaced,
            "the unplaced flag must match what the trial actually released"
        );
    }
    ordered
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

/// The placements contributing the most to the total objective.
///
/// ADR-0025: this used to rank by placement-local `soft` alone, which made it
/// blind to `aggregate` (`MaxOnlineShare`) and `day_mix_cost` — one third of
/// the objective at the time it was measured, since `unplaced` and `aggregate`
/// had moved onto the hard side while this operator kept scoring as if `soft`
/// were still the whole objective. Neither aggregate belongs to a single
/// placement, so `state.aggregate_ruin_score` applies the attribution
/// convention ADR-0025 settled on rather than a delta.
pub(super) fn ruin_worst(
    problem: &Problem,
    current: &Solution,
    state: &SearchState,
    placed: &[PlacementIdx],
    k: usize,
) -> Vec<PlacementIdx> {
    let mut scored: Vec<(PlacementIdx, f64)> = placed
        .iter()
        .map(|&p| {
            let pl = current.get(p).unwrap();
            let o = problem.offering_of(p);
            // The preference and movement costs are included because they ARE
            // placement-local: this operator's whole job is to rank placements
            // by what they cost, and a Session sitting on a slot its lecturer
            // asked to avoid — or away from where a minimize-movement policy
            // wants it — is exactly what it should pick up.
            let capacity = problem.exclusive_capacity(pl.all_rooms());
            let mut cost = pl
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
            if let Some(span) = problem.slots.span(pl.start, o.duration_blocks) {
                let occupant = Occupant::of_offering(o)
                    .with_room(pl.room)
                    .with_additional_rooms(pl.additional_rooms)
                    .with_offering(problem.placement(p).offering);
                cost += state.aggregate_ruin_score(problem, &occupant, &span);
            }
            (p, cost)
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
            // `o.lecturers` is empty for a pool Offering (its lecturers are
            // chosen per-placement, not fixed on the Offering), so a pool
            // Offering is never "lecturer-related" by this heuristic. A
            // search-quality heuristic only — ruining a narrower
            // neighbourhood for a pool Offering costs LNS effectiveness, not
            // correctness, and the actual chosen lecturers are not
            // aggregated anywhere this check could cheaply read.
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

/// Ruin the placed Sessions blocking a cell an UNPLACED Session could use
/// (ADR-0031).
///
/// The other three operators select among PLACED Sessions — by chance, by
/// cost, or by sharing a resource with a uniformly-random anchor — so freeing
/// the exact cell a wedged Offering needs is left to luck, and repair can
/// never evict on its own (an occupied cell scores infinite). This operator
/// starts from the unplaced Session instead: probe a seeded sample of its
/// candidate cells, identify the movable occupants blocking each, and evict
/// the cheapest such set — at most one probe's blockers, never more than the
/// round's ruin cap.
///
/// NOT the fourth arm ADR-0025 rejected. That one would have worked around a
/// fixable scoring inconsistency in `ruin_worst`; an unplaced Session has no
/// placement to score, and what blocks it is a relation between its candidate
/// space and the current occupancy — information no per-placement cost
/// correction can surface, because the blockers may individually be perfectly
/// cheap.
///
/// Blocker identification is a HEURISTIC over the pairwise axes (see
/// [`blocks`]). It may over-approximate — an evicted non-blocker is merely
/// re-placed — and it may miss an axis, wasting the round on a cell that
/// stays blocked. Correctness never depends on it, exactly like
/// `ruin_related`'s own relatedness test; `Occupancy` remains the authority.
fn ruin_blocking(
    problem: &Problem,
    current: &Solution,
    placed: &[PlacementIdx],
    unplaced: &[PlacementIdx],
    cap: usize,
    rng: &mut Rng,
) -> Vec<PlacementIdx> {
    let target = unplaced[rng.below(unplaced.len())];
    let offering = problem.offering_of(target);
    let space = CandidateSpace::new(problem, target);
    if space.total == 0 {
        return Vec::new();
    }

    // Sampling WITH replacement plus dedup, rather than a partial
    // Fisher-Yates: a duplicate probe costs one wasted draw in a heuristic,
    // not a correctness or determinism problem.
    let mut probe_ix: Vec<usize> = if space.total <= tuning::BLOCK_PROBE_CELLS {
        (0..space.total).collect()
    } else {
        (0..tuning::BLOCK_PROBE_CELLS)
            .map(|_| rng.below(space.total))
            .collect()
    };
    probe_ix.sort_unstable();
    probe_ix.dedup();

    // Fewest blockers wins; ties resolve to the earliest probe, which is
    // canonical after the sort above.
    let mut best: Option<(usize, Vec<PlacementIdx>)> = None;

    for &i in &probe_ix {
        let mv = space.at(i);
        let Some(span) = problem.slots.span(mv.to.start, offering.duration_blocks) else {
            continue;
        };
        let candidate = Occupant::of_offering(offering)
            .with_room(mv.to.room)
            .with_additional_rooms(mv.to.additional_rooms)
            .with_pool_lecturers(mv.to.lecturers);
        // A cell no eviction could ever open — vetoed, calendar-closed,
        // protected — has no blockers worth hunting for.
        if SearchState::statically_blocked(problem, &candidate, &span) {
            continue;
        }

        // A span never crosses a day (the grid refuses spills), so its raw
        // slot indices are contiguous — the same fact `Occupancy::week_of`
        // reads a whole span off its first slot by.
        let first = span[0].get();
        let last = span[span.len() - 1].get();

        let mut blockers: Vec<PlacementIdx> = Vec::new();
        for &q in placed {
            let pl_q = current.get(q).expect("`placed` holds only placed Sessions");
            let o_q = problem.offering_of(q);
            let q_first = pl_q.start.get();
            let q_last = q_first + o_q.duration_blocks as usize - 1;
            if q_last < first || last < q_first {
                continue;
            }
            if blocks(problem, &candidate, o_q, pl_q) {
                blockers.push(q);
                if blockers.len() > cap {
                    // More than this round may evict: the cell cannot be
                    // opened, so the exact set no longer matters.
                    break;
                }
            }
        }
        if blockers.is_empty() || blockers.len() > cap {
            // Empty means the cell is either already free — plain repair will
            // take it this round — or blocked by something this heuristic
            // cannot see (fixed occupancy, an aggregate cap). Neither has an
            // eviction to offer.
            continue;
        }
        if best.as_ref().is_none_or(|(n, _)| blockers.len() < *n) {
            best = Some((blockers.len(), blockers));
        }
    }

    best.map(|(_, b)| b).unwrap_or_default()
}

/// Whether a placed Session would block `candidate` on any pairwise axis —
/// the targeting heuristic behind [`ruin_blocking`], deliberately mirroring
/// how `Occupancy` marks and queries without being it.
///
/// Axis by axis: an EXCLUSIVE Room shared by both (ADR-0022's exemption
/// preserved); intersecting lecturers, reading a pool Offering's chosen set
/// from its `Placement` since its `Offering::lecturers` is empty (the trap
/// `ruin_related` documents); the group conflict closure the way the real
/// check runs it — `mark` sets the OTHER side's closure, `is_free` queries
/// this side's own Groups by identity; intersecting attendees; and a shared
/// `DifferentTime` relation row, which `mark` sets unconditionally.
/// `MeetTogether` anchors and the online-concurrency cap are deliberately not
/// mirrored — a missed blocker wastes a round, nothing more.
fn blocks(problem: &Problem, candidate: &Occupant<'_>, o_q: &Offering, pl_q: Placement) -> bool {
    if candidate.enforce.room
        && o_q.enforce.room
        && candidate
            .all_rooms()
            .any(|r| problem.rooms[r.get()].is_exclusive() && pl_q.all_rooms().any(|rq| rq == r))
    {
        return true;
    }
    if candidate.enforce.lecturer
        && o_q.enforce.lecturer
        && candidate.all_lecturers().any(|l| {
            o_q.lecturers.contains(&l) || pl_q.lecturers.iter().flatten().any(|&lq| lq == l)
        })
    {
        return true;
    }
    if candidate.enforce.group
        && o_q.enforce.group
        && o_q
            .conflict_groups
            .iter()
            .any(|g| candidate.own_groups.contains(g))
    {
        return true;
    }
    if candidate.enforce.person
        && o_q.enforce.person
        && o_q
            .attendees
            .iter()
            .any(|a| candidate.attendees.contains(a))
    {
        return true;
    }
    o_q.different_time_relations
        .iter()
        .any(|r| candidate.different_time_relations.contains(r))
}
