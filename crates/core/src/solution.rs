//! Mutable search state, and the occupancy index the hot loop reads.

use crate::aggregates::Aggregates;
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
    pub kind: &'a str,
    pub room: Option<RoomIdx>,
    pub lecturers: &'a [PersonIdx],
    /// Unexpanded. Used to **query**.
    pub own_groups: &'a [GroupIdx],
    /// Expanded through ancestors and descendants. Used to **mark**.
    pub conflict_groups: &'a [GroupIdx],
    pub attendees: &'a [PersonIdx],
    /// `own_groups` expanded downward only — the scope for the two
    /// Group-aggregate types.
    pub subtree_groups: &'a [GroupIdx],
    /// Slots blocked by this Session's lecturers' blackouts. `None` for
    /// immovable occupancy, which is never re-placed.
    pub veto_slots: Option<&'a crate::bitset::BitSet>,
    pub enforce: Enforce,
}

impl<'a> Occupant<'a> {
    pub fn of_offering(o: &'a Offering) -> Self {
        Self {
            kind: &o.kind,
            room: None,
            lecturers: &o.lecturers,
            own_groups: &o.own_groups,
            conflict_groups: &o.conflict_groups,
            attendees: &o.attendees,
            subtree_groups: &o.subtree_groups,
            veto_slots: Some(&o.veto_slots),
            enforce: o.enforce,
        }
    }

    pub fn of_fixed(f: &'a FixedOccupancy) -> Self {
        Self {
            kind: &f.kind,
            room: f.room,
            lecturers: &f.lecturers,
            own_groups: &f.own_groups,
            conflict_groups: &f.conflict_groups,
            attendees: &f.attendees,
            subtree_groups: &f.subtree_groups,
            // Immovable occupancy is never re-placed, so its own blackout mask
            // is irrelevant; it still contributes to every other counter.
            veto_slots: None,
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
    /// The room whose slot bit this Session claims, if any.
    ///
    /// `None` when the Session is unplaced, when `RoomDoubleBooking` is not
    /// configured for its kind, or when the room is **not exclusive** — see
    /// [`Problem`]'s `Room::is_exclusive`. A virtual room's row therefore stays
    /// permanently clear, which is what lets any number of Sessions run online
    /// in the same slot.
    ///
    /// `mark`, `unmark` and `is_free` all go through here rather than reading
    /// `who.room` directly. That is deliberate: if one of them claimed a bit the
    /// others did not test, the search would refuse placements it then declined
    /// to report, or free a bit it never set. There is one expression, so there
    /// is one answer.
    #[inline]
    fn exclusive_room(problem: &Problem, who: &Occupant<'_>) -> Option<RoomIdx> {
        who.room
            .filter(|&r| problem.rooms[r.get()].is_exclusive())
    }

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
                occ.mark(problem, &Occupant::of_fixed(f), &span);
            }
        }
        occ
    }

    /// Mark a Session busy.
    ///
    /// Groups are marked through their **conflict closure** — a cohort-level
    /// Session blocks every descendant class, and a seminar Session blocks its
    /// ancestors. Only one side expands; see [`crate::groups`].
    pub fn mark(&mut self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) {
        let room = Self::exclusive_room(problem, who);
        for &s in span {
            let c = s.get();
            if who.enforce.room && let Some(r) = room {
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

    pub fn unmark(&mut self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) {
        let room = Self::exclusive_room(problem, who);
        for &s in span {
            let c = s.get();
            if who.enforce.room && let Some(r) = room {
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
    pub fn is_free(&self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) -> bool {
        let room = Self::exclusive_room(problem, who);
        for &s in span {
            let c = s.get();
            if who.enforce.room
                && let Some(r) = room
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

/// The search's full incremental index.
///
/// Slot-keyed occupancy is no longer the whole story: slice 4 added
/// day-granularity and window-granularity counters, which cannot be expressed
/// as `(entity, slot)` bitsets. Both live here so the evaluator receives one
/// coherent view of "what is currently true".
#[derive(Clone, Debug)]
pub struct SearchState {
    pub occupancy: Occupancy,
    pub aggregates: Aggregates,
}

impl SearchState {
    /// Seed with everything the solver may not move.
    pub fn from_fixed(problem: &Problem) -> Self {
        let mut state = Self {
            occupancy: Occupancy::new(problem),
            aggregates: problem.aggregate_template.clone(),
        };
        for f in &problem.fixed {
            if let Some(span) = problem.slots.span(f.start, f.duration_blocks) {
                state.mark(problem, &Occupant::of_fixed(f), &span);
            }
        }
        state
    }

    fn is_online(problem: &Problem, room: Option<RoomIdx>) -> bool {
        room.is_some_and(|r| problem.rooms[r.get()].is_virtual)
    }

    fn days_of(problem: &Problem, span: &[SlotIdx]) -> Vec<u32> {
        let mut days: Vec<u32> = span
            .iter()
            .map(|&s| problem.slots.flags(s).day_index)
            .collect();
        days.dedup();
        days
    }

    /// Whether this Session could occupy `span`.
    ///
    /// Covers the four structural types and `LecturerVeto` (a unary slot mask).
    ///
    /// TWO TYPES ARE DELIBERATELY ABSENT, for the same underlying reason:
    /// neither is a question about the candidate alone.
    ///
    /// * `MaxOnlineShare` is a ratio with a moving denominator and cannot be a
    ///   filter without dead-ending construction.
    /// * `OnlineOnsiteSameDay` COULD be one — it is monotone-safe, and it was
    ///   one until the reclassification — but it is now SOFT, so a mixed day is
    ///   priced rather than forbidden.
    ///
    /// Both are scored on the objective instead. See [`crate::aggregates`].
    pub fn is_free(&self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) -> bool {
        if !self.occupancy.is_free(problem, who, span) {
            return false;
        }

        if who.enforce.lecturer_veto
            && let Some(veto) = who.veto_slots
            && span.iter().any(|s| veto.contains(s.get()))
        {
            return false;
        }

        true
    }

    /// Would placing this Session here make a `(group, day)` cell mix delivery
    /// modes that currently does not?
    ///
    /// The day-mix counterpart of [`Self::would_worsen_share`], and used the
    /// same way: the evaluator adds a penalty rather than rejecting the move.
    /// `day_mix_allows` is the same predicate that used to gate `is_free`;
    /// only what the caller does with the answer changed.
    pub fn would_worsen_day_mix(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> bool {
        if !who.enforce.day_mix || who.subtree_groups.is_empty() || span.is_empty() {
            return false;
        }

        let days = Self::days_of(problem, span);
        let online = Self::is_online(problem, who.room);

        !self
            .aggregates
            .day_mix_allows(who.subtree_groups, &days, online)
    }

    /// What the currently mixed days cost, at the configured weight.
    ///
    /// Read off the counters rather than accumulated per placement — a mixed
    /// cell belongs to no single Session, so there is no delta to add when one
    /// moves. Same treatment `share_violations` already gets.
    pub fn day_mix_cost(&self, problem: &Problem) -> f64 {
        if problem.day_mix_weight == 0.0 {
            return 0.0;
        }

        self.aggregates.day_mix_violations() as f64 * problem.day_mix_weight
    }

    pub fn mark(&mut self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) {
        self.occupancy.mark(problem, who, span);
        self.apply_aggregates(problem, who, span, true);
    }

    pub fn unmark(&mut self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) {
        self.occupancy.unmark(problem, who, span);
        self.apply_aggregates(problem, who, span, false);
    }

    fn apply_aggregates(
        &mut self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
        add: bool,
    ) {
        if who.subtree_groups.is_empty() || span.is_empty() {
            return;
        }
        let online = Self::is_online(problem, who.room);

        if who.enforce.day_mix {
            let days = Self::days_of(problem, span);
            if add {
                self.aggregates.add_day_mode(who.subtree_groups, &days, online);
            } else {
                self.aggregates
                    .remove_day_mode(who.subtree_groups, &days, online);
            }
        }

        // Share counters are keyed per rule and gated by the rule's own kind
        // scope, so they are applied unconditionally here.
        let week = problem.slots.flags(span[0]).week;
        self.aggregates
            .apply_share(who.kind, who.subtree_groups, week, online, add);
    }

    #[inline]
    pub fn share_violations(&self) -> u32 {
        self.aggregates.share_violations()
    }

    /// Whether placing this Session here would push a share cell over its
    /// allowance. Scored, not filtered — see [`crate::aggregates`].
    pub fn would_worsen_share(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> bool {
        if who.subtree_groups.is_empty() || span.is_empty() {
            return false;
        }
        let week = problem.slots.flags(span[0]).week;
        self.aggregates.share_would_worsen(
            who.kind,
            who.subtree_groups,
            week,
            Self::is_online(problem, who.room),
        )
    }
}
