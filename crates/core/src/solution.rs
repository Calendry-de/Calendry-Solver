//! Mutable search state, and the occupancy index the hot loop reads.

use crate::bitset::BitMatrix;
use crate::ids::{GroupIdx, PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use crate::problem::{Enforce, FixedOccupancy, Offering, Problem};

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

/// A read-only view of one occupying Session, whether placed or immovable.
#[derive(Copy, Clone, Debug)]
pub struct Occupant<'a> {
    pub room: Option<RoomIdx>,
    pub lecturers: &'a [PersonIdx],
    /// Unexpanded. Used to **query**.
    pub own_groups: &'a [GroupIdx],
    /// Expanded through ancestors and descendants. Used to **mark**.
    pub conflict_groups: &'a [GroupIdx],
    pub attendees: &'a [PersonIdx],
    pub enforce: Enforce,
}

impl<'a> Occupant<'a> {
    pub fn of_offering(o: &'a Offering) -> Self {
        Self {
            room: None,
            lecturers: &o.lecturers,
            own_groups: &o.own_groups,
            conflict_groups: &o.conflict_groups,
            attendees: &o.attendees,
            enforce: o.enforce,
        }
    }

    pub fn of_fixed(f: &'a FixedOccupancy) -> Self {
        Self {
            room: f.room,
            lecturers: &f.lecturers,
            own_groups: &f.own_groups,
            conflict_groups: &f.conflict_groups,
            attendees: &f.attendees,
            enforce: f.enforce,
        }
    }

    pub fn with_room(mut self, room: RoomIdx) -> Self {
        self.room = Some(room);
        self
    }
}

/// Entity-by-slot occupancy for the four structural constraint types.
///
/// Lecturer and attendee are separate matrices even though both are indexed by
/// Person, so `LecturerDoubleBooking` and `PersonDoubleBooking` remain
/// independently switchable — a tenant may enable one without the other.
#[derive(Clone, Debug)]
pub struct Occupancy {
    room: BitMatrix,
    lecturer: BitMatrix,
    attendee: BitMatrix,
    group: BitMatrix,
}

impl Occupancy {
    pub fn new(problem: &Problem) -> Self {
        let slots = problem.slots.len();
        Self {
            room: BitMatrix::new(problem.rooms.len().max(1), slots),
            lecturer: BitMatrix::new(problem.persons.len().max(1), slots),
            attendee: BitMatrix::new(problem.persons.len().max(1), slots),
            group: BitMatrix::new(problem.groups.len().max(1), slots),
        }
    }

    /// Seed with everything the solver may not move: locked, past and
    /// out-of-scope Sessions, plus other tenants' use of Federation-shared
    /// Rooms.
    pub fn from_fixed(problem: &Problem) -> Self {
        let mut occ = Self::new(problem);
        for f in &problem.fixed {
            if let Some(span) = problem.slots.span(f.start, f.duration_blocks) {
                occ.mark(&Occupant::of_fixed(f), &span);
            }
        }
        occ
    }

    /// Mark a Session busy.
    ///
    /// Groups are marked through their **conflict closure** — a cohort-level
    /// Session blocks every descendant class, and a seminar Session blocks its
    /// ancestors. Only one side expands; see [`crate::groups`].
    pub fn mark(&mut self, who: &Occupant<'_>, span: &[SlotIdx]) {
        for &s in span {
            let c = s.get();
            if who.enforce.room && let Some(r) = who.room {
                self.room.set(r.get(), c);
            }
            if who.enforce.lecturer {
                for l in who.lecturers {
                    self.lecturer.set(l.get(), c);
                }
            }
            if who.enforce.group {
                for g in who.conflict_groups {
                    self.group.set(g.get(), c);
                }
            }
            if who.enforce.person {
                for p in who.attendees {
                    self.attendee.set(p.get(), c);
                }
            }
        }
    }

    pub fn unmark(&mut self, who: &Occupant<'_>, span: &[SlotIdx]) {
        for &s in span {
            let c = s.get();
            if who.enforce.room && let Some(r) = who.room {
                self.room.clear(r.get(), c);
            }
            if who.enforce.lecturer {
                for l in who.lecturers {
                    self.lecturer.clear(l.get(), c);
                }
            }
            if who.enforce.group {
                for g in who.conflict_groups {
                    self.group.clear(g.get(), c);
                }
            }
            if who.enforce.person {
                for p in who.attendees {
                    self.attendee.clear(p.get(), c);
                }
            }
        }
    }

    /// Whether this Session could occupy `span` without clashing.
    ///
    /// Groups are queried by **identity**, never expanded. That is what keeps
    /// siblings from colliding: two classes under one cohort share an ancestor,
    /// but neither is in the other's closure.
    pub fn is_free(&self, who: &Occupant<'_>, span: &[SlotIdx]) -> bool {
        for &s in span {
            let c = s.get();
            if who.enforce.room
                && let Some(r) = who.room
                && self.room.get(r.get(), c)
            {
                return false;
            }
            if who.enforce.lecturer && who.lecturers.iter().any(|l| self.lecturer.get(l.get(), c)) {
                return false;
            }
            if who.enforce.group && who.own_groups.iter().any(|g| self.group.get(g.get(), c)) {
                return false;
            }
            if who.enforce.person && who.attendees.iter().any(|p| self.attendee.get(p.get(), c)) {
                return false;
            }
        }
        true
    }
}
