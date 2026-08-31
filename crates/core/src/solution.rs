//! Mutable search state, and the occupancy index the hot loop reads.

use crate::aggregates::Aggregates;
use crate::bitset::BitMatrix;
use crate::ids::{GroupIdx, PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use crate::problem::{Enforce, FixedOccupancy, Offering, Problem};

/// How many Rooms beyond the primary one a single Session can occupy at
/// once — 4 Rooms total. A generous, named cap: nothing in any real
/// institution needs more, and a fixed array (rather than a `Vec`) is what
/// keeps [`Placement`] `Copy` and allocation-free, preserving the exact perf
/// profile the single-Room path already has — `Placement` is passed by value
/// through the search hot path millions of times per run.
pub const MAX_ADDITIONAL_ROOMS: usize = 3;

/// How many lecturers a single Session can be assigned from a genuine
/// candidate pool — see `Offering::has_lecturer_pool`. A generous, named cap
/// mirroring `MAX_ADDITIONAL_ROOMS`: nothing in any real institution needs
/// more chosen lecturers than this on one Session, and a fixed array (rather
/// than a `Vec`) keeps [`Placement`] `Copy` and allocation-free.
///
/// Unrelated to how many lecturers a NON-pool Offering may name —
/// `Offering::lecturers` stays an unbounded `Vec` for that, unchanged,
/// because that case predates this cap and nothing requires one.
pub const MAX_LECTURERS: usize = 4;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Placement {
    pub start: SlotIdx,
    /// The primary Room. Unchanged in meaning from before multi-Room
    /// Sessions existed — every Session has exactly one.
    pub room: RoomIdx,
    /// Additional Rooms this Session ALSO occupies simultaneously, beyond
    /// `room` — `[None; MAX_ADDITIONAL_ROOMS]` for every ordinary,
    /// single-Room Session, which is every Session `Offering.
    /// required_room_count` does not ask for more than one Room for. See
    /// [`Self::all_rooms`].
    pub additional_rooms: [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS],
    /// The lecturers CHOSEN from `Offering::eligible_lecturer_combinations`
    /// for this Session — `[None; MAX_LECTURERS]` for every ordinary Session,
    /// which is every Session whose Offering does not have a genuine
    /// candidate pool (`Offering::has_lecturer_pool`). Read alongside
    /// `Offering::lecturers` through [`Occupant::all_lecturers`], never
    /// alone: a non-pool Offering's lecturers live on the Offering, not here.
    pub lecturers: [Option<PersonIdx>; MAX_LECTURERS],
}

impl Placement {
    /// An ordinary, single-Room placement — `additional_rooms` all `None`.
    /// The overwhelming majority of construction sites want exactly this.
    #[inline]
    pub fn single(start: SlotIdx, room: RoomIdx) -> Self {
        Self {
            start,
            room,
            additional_rooms: [None; MAX_ADDITIONAL_ROOMS],
            lecturers: [None; MAX_LECTURERS],
        }
    }

    /// A placement occupying `room` plus every Room in `additional_rooms`
    /// simultaneously.
    #[inline]
    pub fn with_rooms(
        start: SlotIdx,
        room: RoomIdx,
        additional_rooms: [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS],
    ) -> Self {
        Self { start, room, additional_rooms, lecturers: [None; MAX_LECTURERS] }
    }

    /// Every Room this Session occupies, primary first. The one place "all
    /// of this Session's Rooms" is computed; `Occupancy`'s mark/unmark/
    /// `is_free` and the soft-cost sum both read through this rather than
    /// re-deriving it.
    #[inline]
    pub fn all_rooms(&self) -> impl Iterator<Item = RoomIdx> + '_ {
        std::iter::once(self.room).chain(self.additional_rooms.iter().flatten().copied())
    }
}

#[derive(Clone, Debug)]
pub struct Solution {
    /// Indexed by [`PlacementIdx`]. `None` = not yet placed.
    placements: Vec<Option<Placement>>,
}

impl Solution {
    pub fn empty(problem: &Problem) -> Self {
        Self { placements: vec![None; problem.placements.len()] }
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
    /// `None` for an ad-hoc Session realizing no Offering. Needed only by the
    /// two per-Offering aggregate types (`DistributedPatternAdherence`/
    /// `BlockPatternAdherence`), which key their counters by Offering rather
    /// than by Group or Person.
    pub offering: Option<crate::ids::OfferingIdx>,
    pub scheduling_pattern: crate::problem::SchedulingPattern,
    /// Slots blocked by this Session's lecturers' blackouts. `None` for
    /// immovable occupancy, which is never re-placed.
    pub veto_slots: Option<&'a crate::bitset::BitSet>,
    /// The same, for the blackouts of this Session's Groups and their
    /// ancestors. `None` for the same reason.
    pub group_veto_slots: Option<&'a crate::bitset::BitSet>,
    /// The same, for this Offering's `ProtectedBlock` mask. `None` for the
    /// same reason — immovable occupancy is never re-placed.
    pub protected_block_slots: Option<&'a crate::bitset::BitSet>,
    /// Additional Rooms beyond `room` — see [`Placement::additional_rooms`].
    /// `[None; MAX_ADDITIONAL_ROOMS]` unless set via
    /// [`Self::with_additional_rooms`].
    pub additional_rooms: [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS],
    /// Lecturers CHOSEN from a genuine candidate pool — see
    /// [`Placement::lecturers`]. `[None; MAX_LECTURERS]` unless set via
    /// [`Self::with_pool_lecturers`], which is every Session whose Offering
    /// does not have one (`Offering::has_lecturer_pool`). Read together with
    /// `lecturers` through [`Self::all_lecturers`], never in isolation — a
    /// pool Offering's own `lecturers` is empty, and a non-pool Offering
    /// never populates this field.
    pub pool_lecturers: [Option<PersonIdx>; MAX_LECTURERS],
    pub enforce: Enforce,
    /// Dense row indices of the `DifferentTime` relations this Session's
    /// Offering is a member of — see `Offering::different_time_relations`.
    /// Always present (unlike `offering`, which only a couple of aggregate
    /// types need), because relation membership is checked in the same hot
    /// `mark`/`unmark`/`is_free` path every other occupancy axis goes
    /// through.
    pub different_time_relations: &'a [u32],
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
            offering: None,
            scheduling_pattern: o.scheduling_pattern,
            veto_slots: Some(&o.veto_slots),
            group_veto_slots: Some(&o.group_veto_slots),
            protected_block_slots: Some(&o.protected_block_slots),
            additional_rooms: [None; MAX_ADDITIONAL_ROOMS],
            pool_lecturers: [None; MAX_LECTURERS],
            enforce: o.enforce,
            different_time_relations: &o.different_time_relations,
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
            offering: f.offering,
            scheduling_pattern: f.scheduling_pattern,
            // Immovable occupancy is never re-placed, so its own blackout mask
            // is irrelevant; it still contributes to every other counter.
            veto_slots: None,
            group_veto_slots: None,
            protected_block_slots: None,
            additional_rooms: f.additional_rooms,
            pool_lecturers: [None; MAX_LECTURERS],
            enforce: f.enforce,
            different_time_relations: &f.different_time_relations,
        }
    }

    pub fn with_room(mut self, room: RoomIdx) -> Self {
        self.room = Some(room);
        self
    }

    /// This Session with `additional_rooms` replaced — see
    /// [`Placement::additional_rooms`]. Only a multi-Room Offering's
    /// candidates ever call this; every other caller keeps the default
    /// `[None; MAX_ADDITIONAL_ROOMS]`.
    pub fn with_additional_rooms(
        mut self,
        additional_rooms: [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS],
    ) -> Self {
        self.additional_rooms = additional_rooms;
        self
    }

    /// Every Room this Session occupies, primary first — mirrors
    /// [`Placement::all_rooms`], for the one difference that `room` here is
    /// optional (unplaced construction probes have none yet).
    #[inline]
    pub fn all_rooms(&self) -> impl Iterator<Item = RoomIdx> + '_ {
        self.room
            .into_iter()
            .chain(self.additional_rooms.iter().flatten().copied())
    }

    /// This Session with `pool_lecturers` replaced — see
    /// [`Placement::lecturers`]. Only a pool Offering's candidates ever call
    /// this; every other caller keeps the default `[None; MAX_LECTURERS]`.
    pub fn with_pool_lecturers(
        mut self,
        pool_lecturers: [Option<PersonIdx>; MAX_LECTURERS],
    ) -> Self {
        self.pool_lecturers = pool_lecturers;
        self
    }

    /// Every lecturer this Session has, whichever of the two sources supplies
    /// them — `lecturers` (a non-pool Offering's fixed assignment) and
    /// `pool_lecturers` (a pool Offering's chosen combination) are never both
    /// non-empty for the same Session, so this is simply their union. The one
    /// place "all of this Session's lecturers" is computed; every
    /// lecturer-keyed check reads through here rather than either field
    /// directly.
    #[inline]
    pub fn all_lecturers(&self) -> impl Iterator<Item = PersonIdx> + '_ {
        self.lecturers
            .iter()
            .copied()
            .chain(self.pool_lecturers.iter().flatten().copied())
    }

    /// This Session with `offering` set — see the field's own doc for why
    /// `of_offering` cannot set it directly (it is built from `&Offering`
    /// alone, with no index of its own to attach).
    pub fn with_offering(mut self, offering: crate::ids::OfferingIdx) -> Self {
        self.offering = Some(offering);
        self
    }

    /// This Session with `enforce` replaced.
    ///
    /// For the benchmark harness's per-axis attribution, which has to ask
    /// "would *this one* axis reject the candidate on its own". Prefer
    /// [`Occupant::room_independent_probe`] for the mask the search itself uses.
    pub fn with_enforce(mut self, enforce: Enforce) -> Self {
        self.enforce = enforce;
        self
    }

    /// This Session as a probe over only the axes independent of which Room is
    /// tried, or `None` if no such axis is enforced for its kind.
    ///
    /// Lecturer, group, person, veto (both kinds) and `ProtectedBlock` all
    /// read the slot alone. Only room occupancy and day-mix (which reads the
    /// Room's virtual flag) depend on the Room. Testing the room-independent
    /// axes **once per slot**, before the room loop, is a pure short-circuit:
    /// if they reject, no Room could have rescued the slot.
    ///
    /// One definition, because two callers must agree on it: the constructive
    /// heuristic, and the benchmark harness's construction attribution — whose
    /// entire purpose is reporting *where* construction rejects candidates, and
    /// which can only do that if its filter order matches the heuristic's. It
    /// guaranteed that by holding a verbatim copy of the mask, so adding a
    /// seventh axis would have left it reporting against the old one, silently
    /// and with plausible-looking numbers.
    pub fn room_independent_probe(o: &'a Offering) -> Option<Self> {
        let mut enforce = Enforce { room: false, day_mix: false, ..o.enforce };
        // A pool Offering's lecturers are chosen PER CANDIDATE, so lecturer
        // double-booking is not independent of which choice is being tried
        // the way it is for a fixed assignment — it must be tested inside the
        // room/lecturer loop, on the actual candidate, not hoisted into this
        // once-per-slot probe.
        if o.has_lecturer_pool() {
            enforce.lecturer = false;
        }
        if enforce == Enforce::default() {
            return None;
        }
        Some(Self::of_offering(o).with_enforce(enforce))
    }
}

/// Entity-by-slot occupancy for the four structural constraint types.
///
/// **Private to this module.** It has exactly one consumer, [`SearchState`],
/// which re-exposed three of its five methods with a `&Problem` bolted on; it
/// used to be `pub` and re-exported from the crate root, which put a
/// single-consumer index into the public interface and gave callers a second
/// place to reason about occupancy. Its `from_fixed` also had zero callers while
/// `SearchState::from_fixed` reimplemented the identical seeding rule — two
/// copies of one rule, one of them dead.
///
/// Lecturer and attendee are separate matrices even though both are indexed by
/// Person, so `LecturerDoubleBooking` and `PersonDoubleBooking` remain
/// independently switchable — a tenant may enable one without the other.
#[derive(Clone, Debug)]
struct Occupancy {
    room: BitMatrix,
    lecturer: BitMatrix,
    attendee: BitMatrix,
    group: BitMatrix,
    /// Per-slot count of currently-marked ONLINE Sessions (primary Room
    /// virtual). Meaningful only when [`Problem::max_concurrent_online`] is
    /// `Some`; maintained unconditionally regardless since the counter itself
    /// is cheap and a `bool` guard at every call site would be one more thing
    /// to keep in sync with `is_free`'s own check.
    online: Vec<u32>,
    /// One row per configured `DifferentTime` relation — see
    /// `Problem::different_time_relation_ids`. A bit shared by every member
    /// Offering, exactly like a virtual Room's occupancy row would be shared
    /// by every Session in it: whichever member marks a slot first, the
    /// SAME bit blocks every other member from that slot.
    relation: BitMatrix,
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
    /// Every Room this Session claims a slot bit in — every exclusive Room in
    /// [`Occupant::all_rooms`], primary and additional alike. A virtual
    /// Room's slot never appears, which is what lets any number of Sessions
    /// run online in the same slot; a non-exclusive physical Room (ADR-0022)
    /// is exempt the same way.
    ///
    /// `mark`, `unmark` and `is_free` all go through here rather than reading
    /// `who.room`/`who.additional_rooms` directly. That is deliberate: if one
    /// of them claimed a bit the others did not test, the search would refuse
    /// placements it then declined to report, or free a bit it never set.
    /// There is one expression, so there is one answer.
    #[inline]
    fn exclusive_rooms(
        problem: &Problem,
        who: &Occupant<'_>,
    ) -> [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS + 1] {
        let mut out = [None; MAX_ADDITIONAL_ROOMS + 1];
        for (slot, r) in out.iter_mut().zip(who.all_rooms()) {
            if problem.rooms[r.get()].is_exclusive() {
                *slot = Some(r);
            }
        }
        out
    }

    fn new(problem: &Problem) -> Self {
        let slots = problem.slots.len();
        Self {
            room: BitMatrix::new(problem.rooms.len().max(1), slots),
            lecturer: BitMatrix::new(problem.persons.len().max(1), slots),
            attendee: BitMatrix::new(problem.persons.len().max(1), slots),
            group: BitMatrix::new(problem.groups.len().max(1), slots),
            online: vec![0; slots],
            relation: BitMatrix::new(problem.different_time_relation_ids.len().max(1), slots),
        }
    }

    /// Whether `who`'s PRIMARY Room is virtual — the same "online" reading
    /// `MinimizeOnline`/day-mix already use, and the same reason a multi-Room
    /// Session is never online (ADR: multi-room capacity sums, but "online"
    /// stays a property of the primary Room alone).
    #[inline]
    fn is_online(problem: &Problem, who: &Occupant<'_>) -> bool {
        who.room.is_some_and(|r| problem.rooms[r.get()].is_virtual)
    }

    /// Mark a Session busy.
    ///
    /// Groups are marked through their **conflict closure** — a cohort-level
    /// Session blocks every descendant class, and a seminar Session blocks its
    /// ancestors. Only one side expands; see [`crate::groups`].
    fn mark(&mut self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) {
        let rooms = Self::exclusive_rooms(problem, who);
        let online = Self::is_online(problem, who);
        for &s in span {
            let c = s.get();
            if who.enforce.room {
                for r in rooms.into_iter().flatten() {
                    self.room.set(r.get(), c);
                }
            }
            if who.enforce.lecturer {
                for l in who.all_lecturers() {
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
            if online {
                self.online[c] += 1;
            }
            for &r in who.different_time_relations {
                self.relation.set(r as usize, c);
            }
        }
    }

    fn unmark(&mut self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) {
        let rooms = Self::exclusive_rooms(problem, who);
        let online = Self::is_online(problem, who);
        for &s in span {
            let c = s.get();
            if who.enforce.room {
                for r in rooms.into_iter().flatten() {
                    self.room.clear(r.get(), c);
                }
            }
            if who.enforce.lecturer {
                for l in who.all_lecturers() {
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
            if online {
                debug_assert!(self.online[c] > 0, "unmark must follow a balanced mark");
                self.online[c] -= 1;
            }
            for &r in who.different_time_relations {
                self.relation.clear(r as usize, c);
            }
        }
    }

    /// Whether this Session could occupy `span` without clashing.
    ///
    /// Groups are queried by **identity**, never expanded. That is what keeps
    /// siblings from colliding: two classes under one cohort share an ancestor,
    /// but neither is in the other's closure.
    fn is_free(&self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) -> bool {
        let rooms = Self::exclusive_rooms(problem, who);
        let online_cap = problem
            .max_concurrent_online
            .filter(|_| Self::is_online(problem, who));
        for &s in span {
            let c = s.get();
            if who.enforce.room
                && rooms
                    .into_iter()
                    .flatten()
                    .any(|r| self.room.get(r.get(), c))
            {
                return false;
            }
            if who.enforce.lecturer && who.all_lecturers().any(|l| self.lecturer.get(l.get(), c)) {
                return false;
            }
            if who.enforce.group && who.own_groups.iter().any(|g| self.group.get(g.get(), c)) {
                return false;
            }
            if who.enforce.person && who.attendees.iter().any(|p| self.attendee.get(p.get(), c)) {
                return false;
            }
            if let Some(cap) = online_cap
                && self.online[c] >= cap
            {
                return false;
            }
            if who
                .different_time_relations
                .iter()
                .any(|&r| self.relation.get(r as usize, c))
            {
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
    occupancy: Occupancy,
    pub aggregates: Aggregates,
}

impl SearchState {
    /// Seed with everything the solver may not move: locked, past and
    /// out-of-scope Sessions, plus other tenants' use of Federation-shared
    /// Rooms.
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

    /// Replay a whole solution into a fresh index.
    ///
    /// Used by the from-scratch objective and by the per-iteration drift
    /// assertion. This lived in `search` as `rebuild_state`, which made
    /// [`crate::constraints`] — the *authoritative* hard-constraint check —
    /// depend on the metaheuristic module for what is really a `SearchState`
    /// constructor.
    pub fn replay(problem: &Problem, solution: &Solution) -> Self {
        let mut state = Self::from_fixed(problem);
        for p in problem.placement_ids() {
            if let Some(pl) = solution.get(p) {
                let placed = state.place(problem, p, pl);
                debug_assert!(
                    placed,
                    "a solution recorded a placement whose span does not fit the grid"
                );
            }
        }
        state
    }

    // -----------------------------------------------------------------------
    // Placement primitives
    // -----------------------------------------------------------------------
    //
    // Six sites across three crates used to open-code the same four-line
    // ritual:
    //
    //     let o = problem.offering_of(p);
    //     let occupant = Occupant::of_offering(o).with_room(pl.room);
    //     if let Some(span) = problem.slots.span(pl.start, o.duration_blocks) {
    //         state.mark(problem, &occupant, &span);
    //     }
    //
    // Two things were wrong with that beyond the duplication. `Occupant` and
    // `SlotTable::span` were part of every caller's interface even though one is
    // derived from the `&Problem` the caller already passes. And the `if let`
    // was a **silent no-op on the failure path**: a `None` span skipped the mark
    // while the caller went on to record the placement anyway, leaving the
    // solution holding a placement the occupancy had never heard of. Nothing in
    // the interface said that could not happen; the invariant lived in a comment
    // in a different file.
    //
    // These three are `#[must_use]` so the failure path cannot be dropped
    // without the compiler saying so.

    /// The occupant and span for placement `p` sitting at `at`.
    ///
    /// `None` when the Session would spill past the end of its day, which is the
    /// one case the grid can refuse.
    #[inline]
    fn resolve<'p>(
        problem: &'p Problem,
        p: PlacementIdx,
        at: Placement,
    ) -> Option<(Occupant<'p>, Vec<SlotIdx>)> {
        let o = problem.offering_of(p);
        let span = problem.slots.span(at.start, o.duration_blocks)?;
        let offering = problem.placement(p).offering;
        Some((
            Occupant::of_offering(o)
                .with_room(at.room)
                .with_additional_rooms(at.additional_rooms)
                .with_pool_lecturers(at.lecturers)
                .with_offering(offering),
            span,
        ))
    }

    /// Mark placement `p` busy at `at`.
    ///
    /// Returns `false`, having changed nothing, when the Session would not fit
    /// the grid there.
    #[must_use = "a false return means nothing was marked; the caller must not \
                  record the placement"]
    #[inline]
    pub fn place(&mut self, problem: &Problem, p: PlacementIdx, at: Placement) -> bool {
        match Self::resolve(problem, p, at) {
            Some((occupant, span)) => {
                self.mark(problem, &occupant, &span);
                true
            }
            None => false,
        }
    }

    /// Release placement `p` from `at`. The inverse of [`SearchState::place`].
    #[must_use = "a false return means nothing was released; the index is still marked"]
    #[inline]
    pub fn unplace(&mut self, problem: &Problem, p: PlacementIdx, at: Placement) -> bool {
        match Self::resolve(problem, p, at) {
            Some((occupant, span)) => {
                self.unmark(problem, &occupant, &span);
                true
            }
            None => false,
        }
    }

    /// Whether placement `p` could occupy `at` right now.
    #[must_use]
    #[inline]
    pub fn can_place(&self, problem: &Problem, p: PlacementIdx, at: Placement) -> bool {
        match Self::resolve(problem, p, at) {
            Some((occupant, span)) => self.is_free(problem, &occupant, &span),
            None => false,
        }
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

    /// The distinct `Room.location` indices `who` occupies — `MinimizeLocationChange`'s
    /// own input, deduplicated so a Session split across two Rooms in the same
    /// building counts as touching ONE location, not two.
    fn locations_of(problem: &Problem, who: &Occupant<'_>) -> Vec<u32> {
        let mut locations: Vec<u32> = who.all_rooms().map(|r| problem.room_location(r)).collect();
        locations.sort_unstable();
        locations.dedup();
        locations
    }

    /// The distinct EXCLUSIVE Rooms `who` occupies, as raw indices —
    /// `MinimizeRoomChurn`'s own input. Virtual (non-exclusive) Rooms are
    /// excluded: "home room" variety is a physical-space concept, the same
    /// exemption `Occupancy`'s own Room bitset gives them (ADR-0022).
    fn rooms_of(problem: &Problem, who: &Occupant<'_>) -> Vec<u32> {
        let mut rooms: Vec<u32> = who
            .all_rooms()
            .filter(|r| problem.rooms[r.get()].is_exclusive())
            .map(|r| r.get() as u32)
            .collect();
        rooms.sort_unstable();
        rooms.dedup();
        rooms
    }

    /// Whether this Session could occupy `span`.
    ///
    /// Covers the four structural types, `LecturerVeto`/`GroupVeto` (unary
    /// slot masks), and the calendar closure check below.
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

        // Not a catalogue type, and not gated by any `Enforce` flag or tenant
        // switch: a Break/Holiday week or an individual holiday day is a fact
        // about the calendar, the same kind of always-on rule as the grid
        // refusing a Session that would spill past the end of its day.
        // Existing (fixed) occupancy is untouched — this only gates where a
        // NEW placement may land, the same way locked Sessions are never
        // second-guessed elsewhere in this codebase.
        if span.iter().any(|&s| problem.slots.flags(s).is_closed()) {
            return false;
        }

        if who.enforce.lecturer_veto
            && let Some(veto) = who.veto_slots
            && span.iter().any(|s| veto.contains(s.get()))
        {
            return false;
        }

        // Same shape, separate switch: a tenant may enforce one of the two
        // vetoes without the other, so these cannot share a mask or a flag.
        if who.enforce.group_veto
            && let Some(veto) = who.group_veto_slots
            && span.iter().any(|s| veto.contains(s.get()))
        {
            return false;
        }

        // `ProtectedBlock`: monotone-safe like the calendar closure check
        // above — a reserved slot is never freed by placing something
        // elsewhere — so it is enforced here rather than priced.
        if who.enforce.protected_block
            && let Some(mask) = who.protected_block_slots
            && span.iter().any(|s| mask.contains(s.get()))
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

    /// Would placing this Session here put a second exam-kind Session of
    /// `who`'s Groups on this day? Mirrors [`Self::would_worsen_day_mix`].
    pub fn would_worsen_exam_same_day(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> bool {
        if !who.enforce.exam_spacing_same_day || who.subtree_groups.is_empty() || span.is_empty() {
            return false;
        }
        let days = Self::days_of(problem, span);
        !self
            .aggregates
            .exam_same_day_allows(who.subtree_groups, &days)
    }

    /// What the currently same-day exam clashes cost, at the configured
    /// weight. Mirrors [`Self::day_mix_cost`].
    pub fn exam_same_day_cost(&self, problem: &Problem) -> f64 {
        if problem.exam_same_day_weight == 0.0 {
            return 0.0;
        }
        self.aggregates.exam_same_day_violations() as f64 * problem.exam_same_day_weight
    }

    /// Would placing this Session here put an exam-kind Session of `who`'s
    /// Groups within the configured window of another? Mirrors
    /// [`Self::would_worsen_exam_same_day`].
    pub fn would_worsen_exam_window(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> bool {
        if !who.enforce.exam_spacing_window || who.subtree_groups.is_empty() || span.is_empty() {
            return false;
        }
        let days = Self::days_of(problem, span);
        !self
            .aggregates
            .exam_window_allows(who.subtree_groups, &days)
    }

    /// What the currently clustered exam windows cost, at the configured
    /// weight. Mirrors [`Self::exam_same_day_cost`].
    pub fn exam_window_cost(&self, problem: &Problem) -> f64 {
        if problem.exam_window_weight == 0.0 {
            return 0.0;
        }
        self.aggregates.exam_window_violations() as f64 * problem.exam_window_weight
    }

    /// The `MinimizeWeekdayImbalance` variance DELTA of placing `who` at
    /// `span` — a ranking signal, mirroring [`Self::max_daily_span_delta`].
    pub fn imbalance_delta(&self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) -> f64 {
        if !who.enforce.minimize_weekday_imbalance
            || who.subtree_groups.is_empty()
            || span.is_empty()
        {
            return 0.0;
        }
        let days = Self::days_of(problem, span);
        self.aggregates.imbalance_delta(who.subtree_groups, &days) * problem.imbalance_weight
    }

    /// What every Group's current weekday imbalance costs, at the configured
    /// weight. Read fresh off the counters, like [`Self::day_mix_cost`].
    pub fn imbalance_cost(&self, problem: &Problem) -> f64 {
        self.aggregates.imbalance_cost(problem.imbalance_weight)
    }

    /// The compactness cost DELTA of placing `who` at `span` — a ranking
    /// signal for choosing between repair candidates, not filed as an exact
    /// per-placement charge in `Objective::soft`: like `day_mix_penalty`, it
    /// only has to point the right way, and the authoritative charge is what
    /// `mark`/`unmark` maintain in `Objective::compactness_cost`.
    pub fn compactness_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty() || (!who.enforce.compactness_group && !who.enforce.compactness_person) {
            return 0.0;
        }
        let day = problem.slots.flags(span[0]).day_index;
        let mut delta = 0.0;
        if who.enforce.compactness_group {
            delta += self
                .aggregates
                .group_compactness_delta(who.subtree_groups, day, span) as f64
                * problem.compactness_group_weight;
        }
        if who.enforce.compactness_person {
            delta += self
                .aggregates
                .person_compactness_delta(who.attendees, day, span) as f64
                * problem.compactness_person_weight;
        }
        delta
    }

    /// The `MaxConsecutiveBlocks` cost DELTA of placing `who` at `span` —
    /// mirrors [`Self::compactness_delta`] exactly, run-excess instead of
    /// gap count.
    pub fn max_consecutive_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty()
            || (!who.enforce.max_consecutive_group && !who.enforce.max_consecutive_person)
        {
            return 0.0;
        }
        let day = problem.slots.flags(span[0]).day_index;
        let mut delta = 0.0;
        if who.enforce.max_consecutive_group {
            delta += self
                .aggregates
                .group_run_delta(who.subtree_groups, day, span) as f64
                * problem.max_consecutive_group_weight;
        }
        if who.enforce.max_consecutive_person {
            delta += self.aggregates.person_run_delta(who.attendees, day, span) as f64
                * problem.max_consecutive_person_weight;
        }
        delta
    }

    /// The `MaxDailySpan` cost DELTA of placing `who` at `span` — mirrors
    /// [`Self::max_consecutive_delta`] exactly, span-excess instead of
    /// run-excess.
    pub fn max_daily_span_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty()
            || (!who.enforce.max_daily_span_group && !who.enforce.max_daily_span_person)
        {
            return 0.0;
        }
        let day = problem.slots.flags(span[0]).day_index;
        let mut delta = 0.0;
        if who.enforce.max_daily_span_group {
            delta += self
                .aggregates
                .group_span_delta(who.subtree_groups, day, span) as f64
                * problem.max_daily_span_group_weight;
        }
        if who.enforce.max_daily_span_person {
            delta += self.aggregates.person_span_delta(who.attendees, day, span) as f64
                * problem.max_daily_span_person_weight;
        }
        delta
    }

    /// The `MaxDailySessionCount` cost DELTA of placing `who` at `span` —
    /// mirrors [`Self::max_daily_span_delta`], a raw count-excess instead of
    /// span-excess.
    pub fn max_daily_session_count_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty()
            || (!who.enforce.max_daily_session_count_group
                && !who.enforce.max_daily_session_count_person)
        {
            return 0.0;
        }
        let day = problem.slots.flags(span[0]).day_index;
        let mut delta = 0.0;
        if who.enforce.max_daily_session_count_group {
            delta += self
                .aggregates
                .group_daily_count_delta(who.subtree_groups, day) as f64
                * problem.max_daily_session_count_group_weight;
        }
        if who.enforce.max_daily_session_count_person {
            delta += self.aggregates.person_daily_count_delta(who.attendees, day) as f64
                * problem.max_daily_session_count_person_weight;
        }
        delta
    }

    /// The `MinimizeLocationChange` cost DELTA of placing `who` at `span` —
    /// mirrors [`Self::max_daily_span_delta`], over distinct-location excess
    /// instead of span-excess.
    pub fn location_change_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty()
            || (!who.enforce.minimize_location_change_group
                && !who.enforce.minimize_location_change_person)
        {
            return 0.0;
        }
        let day = problem.slots.flags(span[0]).day_index;
        let locations = Self::locations_of(problem, who);
        let mut delta = 0.0;
        if who.enforce.minimize_location_change_group {
            delta += self
                .aggregates
                .group_location_delta(who.subtree_groups, day, &locations)
                as f64
                * problem.location_change_group_weight;
        }
        if who.enforce.minimize_location_change_person {
            delta += self
                .aggregates
                .person_location_delta(who.attendees, day, &locations) as f64
                * problem.location_change_person_weight;
        }
        delta
    }

    /// The `RoomTurnaroundBuffer` cost DELTA of placing `who` at `span` — the
    /// read-only preview, summed over every EXCLUSIVE Room `who` occupies
    /// (see [`Occupant::all_rooms`]): a multi-Room Session needs a buffer in
    /// each physical Room it uses.
    pub fn room_turnaround_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty() || !who.enforce.room_turnaround {
            return 0.0;
        }
        let day = problem.slots.flags(span[0]).day_index;
        let mut delta = 0i64;
        for r in who.all_rooms() {
            if problem.rooms[r.get()].is_exclusive() {
                delta += self.aggregates.room_turnaround_delta(r, day, span);
            }
        }
        delta as f64 * problem.room_turnaround_weight
    }

    /// The `MinimizeRoomChurn` cost DELTA of placing `who` at `span` — the
    /// read-only preview, mirroring [`Self::location_change_delta`] but keyed
    /// by WEEK and ROOM rather than day and location, and Group-only.
    pub fn room_churn_delta(&self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) -> f64 {
        if span.is_empty() || !who.enforce.minimize_room_churn {
            return 0.0;
        }
        let week = problem.slots.flags(span[0]).week;
        let rooms = Self::rooms_of(problem, who);
        self.aggregates
            .group_churn_delta(who.subtree_groups, week, &rooms) as f64
            * problem.room_churn_weight
    }

    /// The `RoomConsistency` cost DELTA of placing `who` at `span` — the
    /// read-only preview, mirroring [`Self::scheduling_pattern_delta`]: keyed
    /// by Offering with no day/week axis at all, so `who.offering` and
    /// `who.room` (the PRIMARY Room only — an Offering has one "usual" Room,
    /// not a set) are what matter, not `span`'s position.
    pub fn room_consistency_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty() || !who.enforce.room_consistency {
            return 0.0;
        }
        let (Some(offering), Some(room)) = (who.offering, who.room) else {
            return 0.0;
        };
        self.aggregates.room_consistency_delta(offering, room) as f64
            * problem.room_consistency_weight
    }

    /// The `LecturerConsistency` cost DELTA of placing `who` at `span` — the
    /// read-only preview, mirroring [`Self::room_consistency_delta`] but over
    /// `who.all_lecturers()` instead of a single Room, and inert for any
    /// Offering without a genuine lecturer pool.
    pub fn lecturer_consistency_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty() || !who.enforce.lecturer_consistency {
            return 0.0;
        }
        let Some(offering) = who.offering else {
            return 0.0;
        };
        let o = &problem.offerings[offering.get()];
        if !o.has_lecturer_pool() {
            return 0.0;
        }
        let lecturers: Vec<PersonIdx> = who.all_lecturers().collect();
        self.aggregates.lecturer_consistency_delta(
            offering,
            &lecturers,
            o.lecturer_required_count(),
        ) as f64
            * problem.lecturer_consistency_weight
    }

    /// The `MaxOfferingSessionsPerDay` cost DELTA of placing `who` at
    /// `span` — mirrors [`Self::max_daily_session_count_delta`], singular
    /// Offering rather than a Group/Person slice.
    pub fn offering_daily_count_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty() || !who.enforce.max_offering_sessions_per_day {
            return 0.0;
        }
        let Some(offering) = who.offering else {
            return 0.0;
        };
        let day = problem.slots.flags(span[0]).day_index;
        self.aggregates.offering_daily_count_delta(offering, day) as f64
            * problem.max_offering_sessions_per_day_weight
    }

    /// The `MaxConsecutiveOfferingBlocks` cost DELTA of placing `who` at
    /// `span` — mirrors [`Self::max_daily_span_delta`], `offering_run_delta`
    /// instead of the Group/Person span.
    pub fn offering_run_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty() || !who.enforce.max_consecutive_offering_blocks {
            return 0.0;
        }
        let Some(offering) = who.offering else {
            return 0.0;
        };
        let day = problem.slots.flags(span[0]).day_index;
        self.aggregates.offering_run_delta(offering, day, span) as f64
            * problem.max_consecutive_offering_blocks_weight
    }

    /// The `MinimizeOfferingDaySplit` cost DELTA of placing `who` at `span`.
    pub fn offering_split_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty() || !who.enforce.minimize_offering_day_split {
            return 0.0;
        }
        let Some(offering) = who.offering else {
            return 0.0;
        };
        let day = problem.slots.flags(span[0]).day_index;
        self.aggregates.offering_split_delta(offering, day, span) as f64
            * problem.minimize_offering_day_split_weight
    }

    /// The `MaxWeeklyTeachingLoad` cost DELTA of placing `who` at `span` —
    /// the read-only preview, mirroring [`Self::max_daily_span_delta`].
    /// Keyed by `who.all_lecturers()` and the WEEK `span` falls in, not by
    /// day — this type is a weekly cap, not a daily one.
    pub fn max_weekly_teaching_load_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty() || !who.enforce.max_weekly_teaching_load {
            return 0.0;
        }
        let week = problem.slots.flags(span[0]).week;
        self.aggregates
            .teaching_load_delta(who.all_lecturers(), week, span.len() as u32) as f64
            * problem.max_weekly_teaching_load_weight
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
        if span.is_empty() {
            return;
        }

        // Compactness is gated per-axis rather than sharing the day-mix/share
        // early return above: a Person-only Offering (participants with no
        // Group attached) has empty `subtree_groups` but non-empty
        // `attendees`, and gating the Person axis on `subtree_groups` being
        // non-empty would silently drop exactly that Offering's compactness
        // signal.
        let day = problem.slots.flags(span[0]).day_index;
        if who.enforce.compactness_group {
            if add {
                self.aggregates
                    .add_group_compactness(who.subtree_groups, day, span);
            } else {
                self.aggregates
                    .remove_group_compactness(who.subtree_groups, day, span);
            }
        }
        if who.enforce.compactness_person {
            if add {
                self.aggregates
                    .add_person_compactness(who.attendees, day, span);
            } else {
                self.aggregates
                    .remove_person_compactness(who.attendees, day, span);
            }
        }
        if who.enforce.max_consecutive_group {
            if add {
                self.aggregates.add_group_run(who.subtree_groups, day, span);
            } else {
                self.aggregates
                    .remove_group_run(who.subtree_groups, day, span);
            }
        }
        if who.enforce.max_consecutive_person {
            if add {
                self.aggregates.add_person_run(who.attendees, day, span);
            } else {
                self.aggregates.remove_person_run(who.attendees, day, span);
            }
        }
        if who.enforce.max_daily_span_group {
            if add {
                self.aggregates
                    .add_group_span(who.subtree_groups, day, span);
            } else {
                self.aggregates
                    .remove_group_span(who.subtree_groups, day, span);
            }
        }
        if who.enforce.max_daily_span_person {
            if add {
                self.aggregates.add_person_span(who.attendees, day, span);
            } else {
                self.aggregates.remove_person_span(who.attendees, day, span);
            }
        }
        if who.enforce.max_daily_session_count_group {
            if add {
                self.aggregates
                    .add_group_daily_count(who.subtree_groups, day);
            } else {
                self.aggregates
                    .remove_group_daily_count(who.subtree_groups, day);
            }
        }
        if who.enforce.max_daily_session_count_person {
            if add {
                self.aggregates.add_person_daily_count(who.attendees, day);
            } else {
                self.aggregates
                    .remove_person_daily_count(who.attendees, day);
            }
        }
        // Shared substrate: either type wanting this axis is enough to
        // maintain it, since both `MaxDays` and `MaxConsecutiveDays` reduce
        // the SAME day-occupancy cell (see `Aggregates::day_cap_group`/
        // `day_cap_person`) — calling both add functions here would
        // double-count.
        if who.enforce.max_days_group || who.enforce.max_consecutive_days_group {
            if add {
                self.aggregates.add_group_day_cap(who.subtree_groups, day);
            } else {
                self.aggregates
                    .remove_group_day_cap(who.subtree_groups, day);
            }
        }
        if who.enforce.max_days_person || who.enforce.max_consecutive_days_person {
            if add {
                self.aggregates.add_person_day_cap(who.attendees, day);
            } else {
                self.aggregates.remove_person_day_cap(who.attendees, day);
            }
        }
        if who.enforce.max_weekly_teaching_load {
            let week = problem.slots.flags(span[0]).week;
            if add {
                self.aggregates
                    .add_teaching_load(who.all_lecturers(), week, span.len() as u32);
            } else {
                self.aggregates
                    .remove_teaching_load(who.all_lecturers(), week, span.len() as u32);
            }
        }
        if who.enforce.minimize_location_change_group || who.enforce.minimize_location_change_person
        {
            let locations = Self::locations_of(problem, who);
            if who.enforce.minimize_location_change_group {
                if add {
                    self.aggregates
                        .add_group_location(who.subtree_groups, day, &locations);
                } else {
                    self.aggregates
                        .remove_group_location(who.subtree_groups, day, &locations);
                }
            }
            if who.enforce.minimize_location_change_person {
                if add {
                    self.aggregates
                        .add_person_location(who.attendees, day, &locations);
                } else {
                    self.aggregates
                        .remove_person_location(who.attendees, day, &locations);
                }
            }
        }
        if who.enforce.room_turnaround {
            for r in who.all_rooms() {
                if problem.rooms[r.get()].is_exclusive() {
                    if add {
                        self.aggregates.add_room_turnaround(r, day, span);
                    } else {
                        self.aggregates.remove_room_turnaround(r, day, span);
                    }
                }
            }
        }

        // Scheduling pattern: keyed by Offering, so unlike every axis above it
        // needs `who.offering` — `None` for an ad-hoc Session, which has
        // nothing for either pattern to mean. Gated on BOTH the kind-scoped
        // `Enforce` flag AND the Offering's own tagged pattern: an instance
        // enabled for this kind still only prices the Offerings actually
        // tagged for its pattern.
        if let Some(offering) = who.offering {
            use crate::problem::SchedulingPattern;
            if who.enforce.distributed_pattern
                && who.scheduling_pattern == SchedulingPattern::Distributed
            {
                let cell = problem.slots.weekly_cell(span[0]);
                if add {
                    self.aggregates.add_distributed(offering, cell);
                } else {
                    self.aggregates.remove_distributed(offering, cell);
                }
            }
            if who.enforce.block_pattern && who.scheduling_pattern == SchedulingPattern::Block {
                let week = problem.slots.flags(span[0]).week;
                if add {
                    self.aggregates.add_block(offering, week);
                } else {
                    self.aggregates.remove_block(offering, week);
                }
            }
            if who.enforce.room_consistency
                && let Some(room) = who.room
            {
                if add {
                    self.aggregates.add_room_consistency(offering, room);
                } else {
                    self.aggregates.remove_room_consistency(offering, room);
                }
            }
            if who.enforce.lecturer_consistency {
                let o = &problem.offerings[offering.get()];
                if o.has_lecturer_pool() {
                    let lecturers: Vec<PersonIdx> = who.all_lecturers().collect();
                    let required = o.lecturer_required_count();
                    if add {
                        self.aggregates
                            .add_lecturer_consistency(offering, &lecturers, required);
                    } else {
                        self.aggregates
                            .remove_lecturer_consistency(offering, &lecturers, required);
                    }
                }
            }
            if who.enforce.max_offering_sessions_per_day {
                if add {
                    self.aggregates.add_offering_daily_count(offering, day);
                } else {
                    self.aggregates.remove_offering_daily_count(offering, day);
                }
            }
            if who.enforce.max_consecutive_offering_blocks {
                if add {
                    self.aggregates.add_offering_run(offering, day, span);
                } else {
                    self.aggregates.remove_offering_run(offering, day, span);
                }
            }
            if who.enforce.minimize_offering_day_split {
                if add {
                    self.aggregates.add_offering_split(offering, day, span);
                } else {
                    self.aggregates.remove_offering_split(offering, day, span);
                }
            }
        }

        if who.subtree_groups.is_empty() {
            return;
        }
        let online = Self::is_online(problem, who.room);

        if who.enforce.day_mix {
            let days = Self::days_of(problem, span);
            if add {
                self.aggregates
                    .add_day_mode(who.subtree_groups, &days, online);
            } else {
                self.aggregates
                    .remove_day_mode(who.subtree_groups, &days, online);
            }
        }

        if who.enforce.exam_spacing_same_day {
            let days = Self::days_of(problem, span);
            if add {
                self.aggregates.add_exam_same_day(who.subtree_groups, &days);
            } else {
                self.aggregates
                    .remove_exam_same_day(who.subtree_groups, &days);
            }
        }

        if who.enforce.exam_spacing_window {
            let days = Self::days_of(problem, span);
            if add {
                self.aggregates.add_exam_window(who.subtree_groups, &days);
            } else {
                self.aggregates
                    .remove_exam_window(who.subtree_groups, &days);
            }
        }

        if who.enforce.minimize_weekday_imbalance {
            let days = Self::days_of(problem, span);
            if add {
                self.aggregates.add_imbalance(who.subtree_groups, &days);
            } else {
                self.aggregates.remove_imbalance(who.subtree_groups, &days);
            }
        }

        if who.enforce.minimize_room_churn {
            let churn_week = problem.slots.flags(span[0]).week;
            let rooms = Self::rooms_of(problem, who);
            if add {
                self.aggregates
                    .add_group_churn(who.subtree_groups, churn_week, &rooms);
            } else {
                self.aggregates
                    .remove_group_churn(who.subtree_groups, churn_week, &rooms);
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

    #[inline]
    pub fn max_days_violations(&self) -> u32 {
        self.aggregates.max_days_violations()
    }

    #[inline]
    pub fn max_consecutive_days_violations(&self) -> u32 {
        self.aggregates.max_consecutive_days_violations()
    }

    /// What `ruin_worst` should charge this occupant for the currently
    /// violated aggregate cells it sits in — `MaxOnlineShare` and
    /// `OnlineOnsiteSameDay` together.
    ///
    /// Neither aggregate belongs to a single placement (see
    /// [`crate::aggregates`]), so this is an attribution convention, not an
    /// exact delta: see [`Aggregates::share_violation_cost`] and
    /// [`Aggregates::day_mix_violation_cost`] for which occupants are charged
    /// for which breach.
    pub fn aggregate_ruin_score(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        if span.is_empty() {
            return 0.0;
        }

        let mut score = 0.0;

        // Same axis-independence as `apply_aggregates`: a Person-only
        // Offering must still be scored for the Person axis even with no
        // Groups attached.
        if who.enforce.compactness_group || who.enforce.compactness_person {
            let day = problem.slots.flags(span[0]).day_index;
            score += self.aggregates.compactness_ruin_cost(
                who.subtree_groups,
                who.attendees,
                day,
                problem.compactness_group_weight,
                problem.compactness_person_weight,
            );
        }

        if who.enforce.max_consecutive_group || who.enforce.max_consecutive_person {
            let day = problem.slots.flags(span[0]).day_index;
            score += self.aggregates.max_consecutive_ruin_cost(
                who.subtree_groups,
                who.attendees,
                day,
                problem.max_consecutive_group_weight,
                problem.max_consecutive_person_weight,
            );
        }

        if who.enforce.max_daily_span_group || who.enforce.max_daily_span_person {
            let day = problem.slots.flags(span[0]).day_index;
            score += self.aggregates.max_daily_span_ruin_cost(
                who.subtree_groups,
                who.attendees,
                day,
                problem.max_daily_span_group_weight,
                problem.max_daily_span_person_weight,
            );
        }

        if who.enforce.max_daily_session_count_group || who.enforce.max_daily_session_count_person {
            let day = problem.slots.flags(span[0]).day_index;
            score += self.aggregates.max_daily_session_count_ruin_cost(
                who.subtree_groups,
                who.attendees,
                day,
                problem.max_daily_session_count_group_weight,
                problem.max_daily_session_count_person_weight,
            );
        }

        if who.enforce.max_weekly_teaching_load {
            let week = problem.slots.flags(span[0]).week;
            score += self.aggregates.teaching_load_ruin_cost(
                who.all_lecturers(),
                week,
                problem.max_weekly_teaching_load_weight,
            );
        }

        if who.enforce.minimize_location_change_group || who.enforce.minimize_location_change_person
        {
            let day = problem.slots.flags(span[0]).day_index;
            score += self.aggregates.location_change_ruin_cost(
                who.subtree_groups,
                who.attendees,
                day,
                problem.location_change_group_weight,
                problem.location_change_person_weight,
            );
        }

        if who.enforce.room_turnaround {
            let day = problem.slots.flags(span[0]).day_index;
            for r in who.all_rooms() {
                if problem.rooms[r.get()].is_exclusive() {
                    score += self.aggregates.room_turnaround_ruin_cost(
                        r,
                        day,
                        span,
                        problem.room_turnaround_weight,
                    );
                }
            }
        }

        if let Some(offering) = who.offering {
            use crate::problem::SchedulingPattern;
            if who.enforce.distributed_pattern
                && who.scheduling_pattern == SchedulingPattern::Distributed
            {
                score += self
                    .aggregates
                    .distributed_ruin_cost(offering, problem.distributed_pattern_weight);
            }
            if who.enforce.block_pattern && who.scheduling_pattern == SchedulingPattern::Block {
                score += self
                    .aggregates
                    .block_ruin_cost(offering, problem.block_pattern_weight);
            }
            if who.enforce.room_consistency {
                score += self
                    .aggregates
                    .consistency_ruin_cost(offering, problem.room_consistency_weight);
            }
            if who.enforce.lecturer_consistency {
                let o = &problem.offerings[offering.get()];
                if o.has_lecturer_pool() {
                    score += self.aggregates.lecturer_consistency_ruin_cost(
                        offering,
                        o.lecturer_required_count(),
                        problem.lecturer_consistency_weight,
                    );
                }
            }
            if who.enforce.max_offering_sessions_per_day {
                let day = problem.slots.flags(span[0]).day_index;
                score += self.aggregates.offering_daily_count_ruin_cost(
                    offering,
                    day,
                    problem.max_offering_sessions_per_day_weight,
                );
            }
            if who.enforce.max_consecutive_offering_blocks {
                let day = problem.slots.flags(span[0]).day_index;
                score += self.aggregates.offering_run_ruin_cost(
                    offering,
                    day,
                    problem.max_consecutive_offering_blocks_weight,
                );
            }
            if who.enforce.minimize_offering_day_split {
                let day = problem.slots.flags(span[0]).day_index;
                score += self.aggregates.offering_split_ruin_cost(
                    offering,
                    day,
                    problem.minimize_offering_day_split_weight,
                );
            }
        }

        if who.subtree_groups.is_empty() {
            return score;
        }

        if Self::is_online(problem, who.room) {
            let week = problem.slots.flags(span[0]).week;
            score += self.aggregates.share_violation_cost(
                who.kind,
                who.subtree_groups,
                week,
                problem.hard_penalty,
            );
        }

        if who.enforce.day_mix {
            let days = Self::days_of(problem, span);
            score += self.aggregates.day_mix_violation_cost(
                who.subtree_groups,
                &days,
                problem.day_mix_weight,
            );
        }

        if who.enforce.exam_spacing_same_day {
            let days = Self::days_of(problem, span);
            score += self.aggregates.exam_same_day_violation_cost(
                who.subtree_groups,
                &days,
                problem.exam_same_day_weight,
            );
        }

        if who.enforce.exam_spacing_window {
            let days = Self::days_of(problem, span);
            score += self.aggregates.exam_window_violation_cost(
                who.subtree_groups,
                &days,
                problem.exam_window_weight,
            );
        }

        if who.enforce.minimize_weekday_imbalance {
            let days = Self::days_of(problem, span);
            score += self.aggregates.imbalance_ruin_cost(
                who.subtree_groups,
                &days,
                problem.imbalance_weight,
            );
        }

        if who.enforce.minimize_room_churn {
            let week = problem.slots.flags(span[0]).week;
            score += self.aggregates.churn_ruin_cost(
                who.subtree_groups,
                week,
                problem.room_churn_weight,
            );
        }

        score
    }

    /// What the currently idle blocks cost, at the configured weight(s). Read
    /// off the running totals rather than accumulated per placement — a gap
    /// belongs to a day, not to any one Session. Same treatment
    /// `day_mix_cost`/`share_violations` already get.
    pub fn compactness_cost(&self, problem: &Problem) -> f64 {
        if problem.compactness_group_weight == 0.0 && problem.compactness_person_weight == 0.0 {
            return 0.0;
        }
        self.aggregates
            .compactness_cost(problem.compactness_group_weight, problem.compactness_person_weight)
    }

    /// What the currently over-cap runs cost, at the configured weight(s).
    /// Mirrors [`Self::compactness_cost`].
    pub fn max_consecutive_cost(&self, problem: &Problem) -> f64 {
        if problem.max_consecutive_group_weight == 0.0
            && problem.max_consecutive_person_weight == 0.0
        {
            return 0.0;
        }
        self.aggregates.max_consecutive_cost(
            problem.max_consecutive_group_weight,
            problem.max_consecutive_person_weight,
        )
    }

    /// What the currently over-cap daily spans cost, at the configured
    /// weight(s). Mirrors [`Self::max_consecutive_cost`].
    pub fn max_daily_span_cost(&self, problem: &Problem) -> f64 {
        if problem.max_daily_span_group_weight == 0.0 && problem.max_daily_span_person_weight == 0.0
        {
            return 0.0;
        }
        self.aggregates.max_daily_span_cost(
            problem.max_daily_span_group_weight,
            problem.max_daily_span_person_weight,
        )
    }

    /// What the currently over-cap daily Session counts cost, at the
    /// configured weight(s). Mirrors [`Self::max_daily_span_cost`].
    pub fn max_daily_session_count_cost(&self, problem: &Problem) -> f64 {
        if problem.max_daily_session_count_group_weight == 0.0
            && problem.max_daily_session_count_person_weight == 0.0
        {
            return 0.0;
        }
        self.aggregates.max_daily_session_count_cost(
            problem.max_daily_session_count_group_weight,
            problem.max_daily_session_count_person_weight,
        )
    }

    /// What the currently over-cap distinct-location days cost, at the
    /// configured weight(s). Mirrors [`Self::max_daily_span_cost`].
    pub fn location_change_cost(&self, problem: &Problem) -> f64 {
        if problem.location_change_group_weight == 0.0
            && problem.location_change_person_weight == 0.0
        {
            return 0.0;
        }
        self.aggregates.location_change_cost(
            problem.location_change_group_weight,
            problem.location_change_person_weight,
        )
    }

    /// What every currently-violating Room-adjacency boundary costs, at the
    /// configured weight.
    pub fn room_turnaround_cost(&self, problem: &Problem) -> f64 {
        if problem.room_turnaround_weight == 0.0 {
            return 0.0;
        }
        self.aggregates
            .room_turnaround_cost(problem.room_turnaround_weight)
    }

    /// What the currently over-cap distinct-Room weeks cost, at the
    /// configured weight.
    pub fn room_churn_cost(&self, problem: &Problem) -> f64 {
        if problem.room_churn_weight == 0.0 {
            return 0.0;
        }
        self.aggregates.churn_cost(problem.room_churn_weight)
    }

    /// What every currently-inconsistent Offering costs, at the configured
    /// weight.
    pub fn room_consistency_cost(&self, problem: &Problem) -> f64 {
        if problem.room_consistency_weight == 0.0 {
            return 0.0;
        }
        self.aggregates
            .consistency_cost(problem.room_consistency_weight)
    }

    /// What every currently-inconsistent pool Offering's lecturer choice
    /// costs, at the configured weight.
    pub fn lecturer_consistency_cost(&self, problem: &Problem) -> f64 {
        if problem.lecturer_consistency_weight == 0.0 {
            return 0.0;
        }
        self.aggregates
            .lecturer_consistency_cost(problem.lecturer_consistency_weight)
    }

    /// What every currently over-cap `(Offering, day)` Session count costs,
    /// at the configured weight.
    pub fn offering_daily_count_cost(&self, problem: &Problem) -> f64 {
        if problem.max_offering_sessions_per_day_weight == 0.0 {
            return 0.0;
        }
        self.aggregates
            .offering_daily_count_cost(problem.max_offering_sessions_per_day_weight)
    }

    /// What every currently over-cap `(Offering, day)` consecutive run
    /// costs, at the configured weight.
    pub fn offering_run_cost(&self, problem: &Problem) -> f64 {
        if problem.max_consecutive_offering_blocks_weight == 0.0 {
            return 0.0;
        }
        self.aggregates
            .offering_run_cost(problem.max_consecutive_offering_blocks_weight)
    }

    /// What every currently-split `(Offering, day)` costs, at the configured
    /// weight.
    pub fn offering_split_cost(&self, problem: &Problem) -> f64 {
        if problem.minimize_offering_day_split_weight == 0.0 {
            return 0.0;
        }
        self.aggregates
            .offering_split_cost(problem.minimize_offering_day_split_weight)
    }

    /// What the currently over-cap weekly teaching loads cost, at the
    /// configured weight.
    pub fn max_weekly_teaching_load_cost(&self, problem: &Problem) -> f64 {
        if problem.max_weekly_teaching_load_weight == 0.0 {
            return 0.0;
        }
        self.aggregates
            .teaching_load_cost(problem.max_weekly_teaching_load_weight)
    }

    /// What every Offering's scheduling-pattern adherence currently costs, at
    /// the configured weights. Same read-off-a-running-total treatment as
    /// `compactness_cost`.
    pub fn scheduling_pattern_cost(&self, problem: &Problem) -> f64 {
        self.aggregates
            .distributed_cost(problem.distributed_pattern_weight)
            + self.aggregates.block_cost(problem.block_pattern_weight)
    }

    /// The scheduling-pattern cost DELTA of placing `who` at `span` — a
    /// ranking signal for repair candidates, exactly like
    /// `compactness_delta`: not filed as an exact per-placement charge, only
    /// pointing the right way. The authoritative charge is what `mark`/
    /// `unmark` maintain in `Objective::scheduling_pattern_cost`.
    pub fn scheduling_pattern_delta(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> f64 {
        let Some(offering) = who.offering else {
            return 0.0;
        };
        if span.is_empty() {
            return 0.0;
        }
        use crate::problem::SchedulingPattern;
        match who.scheduling_pattern {
            SchedulingPattern::Distributed if who.enforce.distributed_pattern => {
                let cell = problem.slots.weekly_cell(span[0]);
                self.aggregates.distributed_delta(offering, cell) as f64
                    * problem.distributed_pattern_weight
            }
            SchedulingPattern::Block if who.enforce.block_pattern => {
                let week = problem.slots.flags(span[0]).week;
                self.aggregates.block_delta(offering, week) as f64 * problem.block_pattern_weight
            }
            _ => 0.0,
        }
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

    /// HARD, like `would_worsen_share`: a ranking signal only, `false`
    /// whenever neither axis is enforced for `who`'s kind. `MaxDays` and
    /// `MaxConsecutiveDays` share this one preview because a caller wanting
    /// either always wants the newly-occupied day checked the same way —
    /// each reduction (`consecutive`) reads its own threshold.
    fn would_worsen_day_cap(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
        group_enforced: bool,
        person_enforced: bool,
        consecutive: bool,
    ) -> bool {
        if span.is_empty() || (!group_enforced && !person_enforced) {
            return false;
        }
        let day = problem.slots.flags(span[0]).day_index;
        let worsens_group = group_enforced
            && (if consecutive {
                self.aggregates
                    .group_max_consecutive_days_would_worsen(who.subtree_groups, day)
            } else {
                self.aggregates
                    .group_max_days_would_worsen(who.subtree_groups, day)
            });
        let worsens_person = person_enforced
            && (if consecutive {
                self.aggregates
                    .person_max_consecutive_days_would_worsen(who.attendees, day)
            } else {
                self.aggregates
                    .person_max_days_would_worsen(who.attendees, day)
            });
        worsens_group || worsens_person
    }

    pub fn would_worsen_max_days(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> bool {
        self.would_worsen_day_cap(
            problem,
            who,
            span,
            who.enforce.max_days_group,
            who.enforce.max_days_person,
            false,
        )
    }

    pub fn would_worsen_max_consecutive_days(
        &self,
        problem: &Problem,
        who: &Occupant<'_>,
        span: &[SlotIdx],
    ) -> bool {
        self.would_worsen_day_cap(
            problem,
            who,
            span,
            who.enforce.max_consecutive_days_group,
            who.enforce.max_consecutive_days_person,
            true,
        )
    }
}
