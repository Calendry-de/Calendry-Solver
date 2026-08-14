//! The move-evaluation boundary.
//!
//! This trait is the one concession to the deferred GPU backend. It is
//! **batched by construction**: a per-move `score(&Move) -> Score` signature
//! would make a GPU implementation pointless, since the entire value there is
//! scoring thousands of LNS candidate moves per dispatch. Keeping the batch in
//! the signature means a future backend plugs in without the search changing.
//!
//! v1 ships only [`CpuEvaluator`]. No metaheuristic drives this yet — the v1
//! slice uses the constructive heuristic alone — but the boundary exists and is
//! exercised, rather than being an empty promise.

use rayon::prelude::*;

use crate::ids::PlacementIdx;
use crate::problem::Problem;
use crate::solution::{Placement, RoomOccupancy, Solution};

/// A candidate relocation of one placement.
#[derive(Copy, Clone, Debug)]
pub struct Move {
    pub placement: PlacementIdx,
    pub to: Placement,
}

/// Lower is better. Currently counts hard-constraint violations the move would
/// introduce; the weighted soft objective joins this in a later slice.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd, Default)]
pub struct Score(pub f64);

pub trait MoveEvaluator: Send + Sync {
    fn score_batch(
        &self,
        problem: &Problem,
        solution: &Solution,
        occupancy: &RoomOccupancy,
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
        occupancy: &RoomOccupancy,
        moves: &[Move],
        out: &mut [Score],
    ) {
        assert_eq!(moves.len(), out.len(), "moves and out must be equal length");

        out.par_iter_mut()
            .zip(moves.par_iter())
            .for_each(|(slot, mv)| {
                *slot = score_one(problem, solution, occupancy, mv);
            });
    }
}

fn score_one(
    problem: &Problem,
    solution: &Solution,
    occupancy: &RoomOccupancy,
    mv: &Move,
) -> Score {
    let offering = problem.offering_of(mv.placement);

    let Some(span) = problem.slots.span(mv.to.start, offering.duration_blocks) else {
        // Would spill past the end of its day: not representable.
        return Score(f64::INFINITY);
    };

    if !offering.eligible_rooms.contains(&mv.to.room) {
        return Score(f64::INFINITY);
    }

    // Count room conflicts the move would introduce, discounting the placement's
    // own current occupancy since moving it vacates that.
    let current = solution.get(mv.placement);
    let mut conflicts = 0u32;

    for &s in &span {
        if !occupancy.is_busy(mv.to.room, s) {
            continue;
        }
        let self_occupied = current.is_some_and(|c| {
            c.room == mv.to.room
                && problem
                    .slots
                    .span(c.start, offering.duration_blocks)
                    .is_some_and(|own| own.contains(&s))
        });
        if !self_occupied {
            conflicts += 1;
        }
    }

    Score(conflicts as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn batch_scoring_flags_occupied_targets() {
        // 1 offering x 1 session, 2 rooms, 1 slot; room 0 already taken.
        let problem = testing::tiny_problem();
        let solution = Solution::empty(&problem);
        let mut occ = RoomOccupancy::from_fixed(&problem);
        let slot = problem.slots.resolve(0, 1, 0).unwrap();
        occ.occupy(crate::ids::RoomIdx(0), slot);

        let moves = vec![
            Move { placement: PlacementIdx(0), to: Placement { start: slot, room: crate::ids::RoomIdx(0) } },
            Move { placement: PlacementIdx(0), to: Placement { start: slot, room: crate::ids::RoomIdx(1) } },
        ];
        let mut out = vec![Score::default(); moves.len()];

        CpuEvaluator.score_batch(&problem, &solution, &occ, &moves, &mut out);

        assert_eq!(out[0], Score(1.0), "occupied room should score a conflict");
        assert_eq!(out[1], Score(0.0), "free room should score clean");
    }
}
