//! The recreate half of LNS: candidate-space addressing, sampling, scoring,
//! and the exhaustive feasibility fallback for unplaced Sessions (ADR-0031).

use std::collections::HashMap;

use crate::evaluator::{Move, MoveEvaluator, Score};
use crate::ids::PlacementIdx;
use crate::problem::{Offering, Problem};
use crate::rng::Rng;
use crate::solution::{Placement, SearchState, Solution};

use super::tuning;

pub(super) struct Repaired {
    pub(super) best: Option<Placement>,
    /// Post-sampling: what `score_batch` actually saw.
    pub(super) evaluated: u64,
    /// Pre-sampling: the full `slots x eligible_rooms` cross product.
    pub(super) enumerated: u64,
}

/// One placement's candidate space, addressed BY INDEX, never materialized.
///
/// Index `i` is slot-major, then room, then lecturer innermost:
/// `(nth_start(i / width), room_choice((i % width) / n_lecturers),
/// lecturer_choice((i % width) % n_lecturers))` — the order a triple
/// nested slot-then-room-then-lecturer loop would produce.
///
/// One definition, two consumers — [`repair_one`] and [`ruin_blocking`] — so
/// the cell an eviction was planned for is exactly the cell repair can then
/// address.
pub(super) struct CandidateSpace<'p> {
    problem: &'p Problem,
    offering: &'p Offering,
    placement: PlacementIdx,
    n_lecturers: usize,
    width: usize,
    pub(super) total: usize,
}

impl<'p> CandidateSpace<'p> {
    pub(super) fn new(problem: &'p Problem, p: PlacementIdx) -> Self {
        let offering = problem.offering_of(p);
        let n_rooms = offering.room_choice_count();
        // `1` for every non-pool Offering — nothing to choose between, so this
        // adds no width to the cross product and `at()` degenerates back to
        // plain `(start, room)` indexing.
        let n_lecturers = offering.lecturer_choice_count();
        let n_starts = problem.slots.start_count(offering.duration_blocks);
        let width = n_rooms * n_lecturers;
        Self { problem, offering, placement: p, n_lecturers, width, total: n_starts * width }
    }

    pub(super) fn at(&self, i: usize) -> Move {
        let (room, additional_rooms) = self
            .offering
            .room_choice((i % self.width) / self.n_lecturers);
        let lecturers = self
            .offering
            .lecturer_choice((i % self.width) % self.n_lecturers);
        Move {
            placement: self.placement,
            to: Placement {
                start: self
                    .problem
                    .slots
                    .nth_start(self.offering.duration_blocks, i / self.width)
                    .expect("index below start_count"),
                room,
                additional_rooms,
                lecturers,
            },
        }
    }
}

/// Best score, then a SEEDED choice among everything tied with it. `None` when
/// nothing scored finite.
///
/// Breaking ties by lowest index instead would make repair fully
/// deterministic given a candidate list — and that collapses the
/// neighbourhood: ruining the same Session always regenerates the same
/// placement, so LNS can never escape a tie-induced dead end. (Observed for
/// real: a Group forced onto one day would keep re-picking the virtual room
/// and leave its second Session permanently unplaced.) The RNG is consumed
/// sequentially here like everywhere else, so the run stays reproducible.
fn pick_best(scores: &[Score], rng: &mut Rng) -> Option<usize> {
    let mut best_score = f64::INFINITY;
    for s in scores {
        if s.0.is_finite() && s.0 < best_score {
            best_score = s.0;
        }
    }
    if !best_score.is_finite() {
        return None;
    }

    let tied: Vec<usize> = scores
        .iter()
        .enumerate()
        .filter(|(_, s)| s.0 <= best_score + f64::EPSILON)
        .map(|(i, _)| i)
        .collect();
    Some(tied[rng.below(tied.len())])
}

/// Score every eligible `(slot, room)` for one removed Session as a batch, and
/// take the cheapest feasible one.
///
/// `was_unplaced` marks a Session that was ALREADY unplaced before this round
/// (as opposed to one this round's ruin released), and buys it the exhaustive
/// fallback below — the population where a sampling miss is most expensive.
pub(super) fn repair_one<E: MoveEvaluator>(
    problem: &Problem,
    evaluator: &E,
    state: &SearchState,
    solution: &Solution,
    p: PlacementIdx,
    was_unplaced: bool,
    rng: &mut Rng,
) -> Repaired {
    let space = CandidateSpace::new(problem, p);
    let total = space.total;
    if total == 0 {
        return Repaired { best: None, evaluated: 0, enumerated: 0 };
    }
    let mut enumerated = total as u64;

    let keep = total.min(tuning::MAX_CANDIDATES);
    let mut candidates: Vec<Move> = Vec::with_capacity(keep);

    if total <= tuning::MAX_CANDIDATES {
        candidates.extend((0..total).map(|i| space.at(i)));
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
            candidates.push(space.at(picked));
            // Position i is finalized once visited and never read again, so only
            // position j needs recording.
            moved.insert(j, displaced);
        }
        // Restore a canonical order so argmin ties break identically.
        candidates.sort_by_key(|m| (m.to.start.0, m.to.room.0, m.to.lecturers));
    }

    let mut scores = vec![Score::default(); candidates.len()];
    evaluator.score_batch(problem, solution, state, &candidates, &mut scores);
    let mut evaluated = candidates.len() as u64;

    if let Some(pick) = pick_best(&scores, rng) {
        return Repaired { best: Some(candidates[pick].to), evaluated, enumerated };
    }

    // Exhaustive feasibility fallback (ADR-0031), for a Session that was
    // ALREADY unplaced and whose candidate space was SAMPLED: the sampling
    // above happens before feasibility, so all `keep` samples can be occupied
    // while a free cell sits outside the sample — and repair would then report
    // "no placement exists" for a Session that had a home. The `is_free` bit
    // tests over the full space are far cheaper than scoring it, and they run
    // only here, where a miss costs a placement rather than a preference.
    if !was_unplaced || total <= tuning::MAX_CANDIDATES {
        return Repaired { best: None, evaluated, enumerated };
    }
    enumerated += total as u64;

    // Reservoir sampling (algorithm R) over the FEASIBLE cells, so the RNG is
    // consumed once per feasible cell beyond the cap — sequential and
    // reproducible like every other draw.
    let mut feasible: Vec<usize> = Vec::with_capacity(tuning::MAX_CANDIDATES);
    let mut seen = 0usize;
    for i in 0..total {
        if !crate::evaluator::is_feasible(problem, state, &space.at(i)) {
            continue;
        }
        seen += 1;
        if feasible.len() < tuning::MAX_CANDIDATES {
            feasible.push(i);
        } else {
            let j = rng.below(seen);
            if j < tuning::MAX_CANDIDATES {
                feasible[j] = i;
            }
        }
    }
    if feasible.is_empty() {
        // A true statement now, not a sampling artifact: no free cell exists
        // for this Session in the current occupancy.
        return Repaired { best: None, evaluated, enumerated };
    }
    // Canonical order so argmin ties break identically.
    feasible.sort_unstable();

    let candidates: Vec<Move> = feasible.into_iter().map(|i| space.at(i)).collect();
    let mut scores = vec![Score::default(); candidates.len()];
    evaluator.score_batch(problem, solution, state, &candidates, &mut scores);
    evaluated += candidates.len() as u64;

    match pick_best(&scores, rng) {
        Some(pick) => Repaired { best: Some(candidates[pick].to), evaluated, enumerated },
        // Feasibility can shift between the scan and the scoring only if the
        // state changed in between, which it cannot within one call — but a
        // `None` here must stay a graceful miss, not a panic.
        None => Repaired { best: None, evaluated, enumerated },
    }
}
