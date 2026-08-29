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
            offering: None,
            scheduling_pattern: o.scheduling_pattern,
            veto_slots: Some(&o.veto_slots),
            group_veto_slots: Some(&o.group_veto_slots),
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
            offering: f.offering,
            scheduling_pattern: f.scheduling_pattern,
            // Immovable occupancy is never re-placed, so its own blackout mask
            // is irrelevant; it still contributes to every other counter.
            veto_slots: None,
            group_veto_slots: None,
            enforce: f.enforce,
        }
    }

    pub fn with_room(mut self, room: RoomIdx) -> Self {
        self.room = Some(room);
        self
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
    /// Four of the six axes — lecturer, group, person, veto — read the slot
    /// alone. Only room occupancy and day-mix (which reads the Room's virtual
    /// flag) depend on the Room. Testing the four **once per slot**, before the
    /// room loop, is a pure short-circuit: if they reject, no Room could have
    /// rescued the slot.
    ///
    /// One definition, because two callers must agree on it: the constructive
    /// heuristic, and the benchmark harness's construction attribution — whose
    /// entire purpose is reporting *where* construction rejects candidates, and
    /// which can only do that if its filter order matches the heuristic's. It
    /// guaranteed that by holding a verbatim copy of the mask, so adding a
    /// seventh axis would have left it reporting against the old one, silently
    /// and with plausible-looking numbers.
    pub fn room_independent_probe(o: &'a Offering) -> Option<Self> {
        let enforce = Enforce { room: false, day_mix: false, ..o.enforce };
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
        who.room.filter(|&r| problem.rooms[r.get()].is_exclusive())
    }

    fn new(problem: &Problem) -> Self {
        let slots = problem.slots.len();
        Self {
            room: BitMatrix::new(problem.rooms.len().max(1), slots),
            lecturer: BitMatrix::new(problem.persons.len().max(1), slots),
            attendee: BitMatrix::new(problem.persons.len().max(1), slots),
            group: BitMatrix::new(problem.groups.len().max(1), slots),
        }
    }

    /// Mark a Session busy.
    ///
    /// Groups are marked through their **conflict closure** — a cohort-level
    /// Session blocks every descendant class, and a seminar Session blocks its
    /// ancestors. Only one side expands; see [`crate::groups`].
    fn mark(&mut self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) {
        let room = Self::exclusive_room(problem, who);
        for &s in span {
            let c = s.get();
            if who.enforce.room
                && let Some(r) = room
            {
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

    fn unmark(&mut self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) {
        let room = Self::exclusive_room(problem, who);
        for &s in span {
            let c = s.get();
            if who.enforce.room
                && let Some(r) = room
            {
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
    fn is_free(&self, problem: &Problem, who: &Occupant<'_>, span: &[SlotIdx]) -> bool {
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
}
