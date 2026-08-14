//! Mutable search state, and the room-occupancy index the hot loop reads.

use crate::ids::{PlacementIdx, RoomIdx, SlotIdx};
use crate::problem::Problem;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Placement {
    pub start: SlotIdx,
    pub room: RoomIdx,
}

#[derive(Clone, Debug)]
pub struct Solution {
    /// Indexed by [`PlacementIdx`]. `None` = not yet placed.
    placements: Vec<Option<Placement>>,
}

impl Solution {
    pub fn empty(problem: &Problem) -> Self {
        Self {
            placements: vec![None; problem.placements.len()],
        }
    }

    #[inline]
    pub fn get(&self, p: PlacementIdx) -> Option<Placement> {
        self.placements[p.get()]
    }

    #[inline]
    pub fn set(&mut self, p: PlacementIdx, placement: Option<Placement>) {
        self.placements[p.get()] = placement;
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.placements.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    pub fn placed_count(&self) -> usize {
        self.placements.iter().filter(|p| p.is_some()).count()
    }
}

/// Room × slot occupancy as a bitset.
///
/// Room-major so that a single room's week is contiguous in memory, which is
/// the access pattern the constructive heuristic and every candidate move use.
#[derive(Clone, Debug)]
pub struct RoomOccupancy {
    words: Vec<u64>,
    n_slots: usize,
    words_per_room: usize,
}

impl RoomOccupancy {
    pub fn new(n_rooms: usize, n_slots: usize) -> Self {
        let words_per_room = n_slots.div_ceil(64);
        Self {
            words: vec![0; n_rooms * words_per_room],
            n_slots,
            words_per_room,
        }
    }

    #[inline]
    fn addr(&self, room: RoomIdx, slot: SlotIdx) -> (usize, u64) {
        let bit = slot.get();
        debug_assert!(bit < self.n_slots);
        (
            room.get() * self.words_per_room + bit / 64,
            1u64 << (bit % 64),
        )
    }

    #[inline]
    pub fn is_busy(&self, room: RoomIdx, slot: SlotIdx) -> bool {
        let (w, mask) = self.addr(room, slot);
        self.words[w] & mask != 0
    }

    #[inline]
    pub fn occupy(&mut self, room: RoomIdx, slot: SlotIdx) {
        let (w, mask) = self.addr(room, slot);
        self.words[w] |= mask;
    }

    #[inline]
    pub fn release(&mut self, room: RoomIdx, slot: SlotIdx) {
        let (w, mask) = self.addr(room, slot);
        self.words[w] &= !mask;
    }

    /// True if every slot in `span` is free for `room`.
    pub fn span_free(&self, room: RoomIdx, span: &[SlotIdx]) -> bool {
        span.iter().all(|&s| !self.is_busy(room, s))
    }

    /// Seed with everything the solver may not move: locked, past and
    /// out-of-scope Sessions, plus other tenants' use of Federation-shared
    /// Rooms.
    pub fn from_fixed(problem: &Problem) -> Self {
        let mut occ = Self::new(problem.rooms.len(), problem.slots.len());
        for f in &problem.fixed {
            let Some(room) = f.room else { continue };
            if let Some(span) = problem.slots.span(f.start, f.duration_blocks) {
                for s in span {
                    occ.occupy(room, s);
                }
            }
        }
        occ
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitset_round_trips_across_word_boundaries() {
        let mut occ = RoomOccupancy::new(2, 130);
        for slot in [0usize, 63, 64, 65, 129] {
            let s = SlotIdx(slot as u32);
            assert!(!occ.is_busy(RoomIdx(1), s));
            occ.occupy(RoomIdx(1), s);
            assert!(occ.is_busy(RoomIdx(1), s));
            // Rooms must not alias each other.
            assert!(!occ.is_busy(RoomIdx(0), s));
            occ.release(RoomIdx(1), s);
            assert!(!occ.is_busy(RoomIdx(1), s));
        }
    }
}
