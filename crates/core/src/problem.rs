//! The immutable problem instance the search runs against.
//!
//! Built once per run from a caller snapshot, then never mutated. All string ids
//! have been resolved to dense indices by this point, and every derived set
//! (group closures, attendee lists) is precomputed here rather than in the hot
//! loop.

use crate::aggregates::{
    Aggregates, CompactnessInstance, DayMixInstance, DaybreakInstance, ExamSpacingSameDayInstance,
    ExamSpacingWindowInstance, LecturerConsistencyInstance, MaxConsecutiveDaysInstance,
    MaxConsecutiveInstance, MaxConsecutiveOfferingBlocksInstance, MaxDailySessionCountInstance,
    MaxDailySpanInstance, MaxDaysInstance, MaxOfferingSessionsPerDayInstance,
    MaxWeeklyTeachingLoadInstance, MinimizeLocationChangeInstance,
    MinimizeOfferingDaySplitInstance, MinimizeRoomChurnInstance, MinimizeWeekdayImbalanceInstance,
    PatternAdherenceInstance, RoomConsistencyInstance, RoomTurnaroundBufferInstance, ShareInstance,
    TravelTimeInstance,
};
use crate::bitset::BitSet;
use crate::groups::{GroupClosure, GroupCycle};
use crate::ids::{GroupIdx, OfferingIdx, PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use crate::preferences::{Preference, PreferenceInstance, PreferenceModel};
use crate::slots::{GridTime, SlotTable, WeekKind};
use crate::soft::{SoftInstance, SoftModel};
use crate::solution::{MAX_ADDITIONAL_ROOMS, MAX_LECTURERS, Placement};

#[derive(Clone, Debug)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub capacity: u32,
    /// Higher = more premium / scarce.
    pub rank: u32,
    pub is_virtual: bool,
    pub features: Vec<String>,
    /// Scarce by FUNCTION — a lab, a computer room, a workshop. A SEPARATE
    /// axis from [`Self::rank`], which is ordinal desirability and whose
    /// `MinimizeRoomRank.invert` mode reads it in the opposite direction; a
    /// Room can be both, or either alone. Inert until a
    /// `MinimizeSpecializedRoomUse` instance is configured, and even then only
    /// for Offerings that require none of this Room's `features`.
    pub is_specialized: bool,
    pub federation_owned: bool,
    /// Free-text building/campus identifier. `""` means unconfigured —
    /// naturally inert for `MinimizeLocationChange`, since every Room sharing
    /// that empty string counts as the SAME location rather than as distinct
    /// ones.
    pub location: String,
    /// Physical FOOTPRINTS this Room occupies — tenant-defined open
    /// vocabulary, like [`Self::features`]. Two Rooms sharing any tag share a
    /// physical space, so booking either occupies both.
    ///
    /// The movable-wall case: 1.0, 1.1 and 1.2 behind folding partitions are
    /// three bookable Rooms closed and one Audimax open, and all four Room
    /// rows carry one tag. A tag is symmetric by construction — membership,
    /// not a directed reference — so "A blocks B" and "B blocks A" cannot
    /// drift apart, and a Room may carry several, which is how a wall shared
    /// between two combination options is said.
    ///
    /// This is NOT [`Self::is_exclusive`], which is one Room against ITSELF
    /// across time. It is a relationship BETWEEN Rooms, and it is structural
    /// and HARD for the same reason: two Sessions in one physical space at one
    /// time is not a preference to weigh.
    ///
    /// Resolved once into [`Problem::footprint_siblings`], which is what the
    /// hot loop reads; a tag that no other exclusive Room carries resolves to
    /// an empty sibling list and costs nothing.
    pub footprints: Vec<String>,
}

impl Room {
    /// Whether this Room can host only ONE Session per slot.
    ///
    /// True for a physical room, where two Sessions in the same place at the
    /// same time is the definition of a double booking. False for a virtual
    /// one: online delivery is modeled AS a Room so that room-assignment logic
    /// stays uniform, not to make concurrency a scarce resource. Two lectures
    /// streaming at the same hour are not a clash, and there is exactly one
    /// virtual room per delivery mode — so treating it as exclusive caps ALL
    /// online teaching at one Session per slot, institution-wide.
    ///
    /// This is the single definition of that policy. `Occupancy` decides
    /// whether to claim a room's slot bit through it, and `constraints::
    /// check_pair` decides whether to report a shared room through it, so the
    /// search and the report cannot disagree about which rooms are exclusive.
    ///
    /// Note what this is NOT: a virtual room with a genuine concurrency limit
    /// (a single meeting licence, say) cannot be expressed today at all —
    /// `capacity` means seats, and it still gates ELIGIBILITY in `convert`.
    /// Expressing a real cap needs its own field, not an overload of this flag.
    #[inline]
    pub fn is_exclusive(&self) -> bool {
        !self.is_virtual
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
    pub parent: Option<GroupIdx>,
    pub name: String,
    pub size: u32,
    /// Windows in which this Group is unavailable, enforced by `GroupVeto`.
    ///
    /// INHERITS DOWNWARD: this binds the Group and its descendants, never its
    /// ancestors — see [`crate::groups::GroupClosure::expand_ancestry`]. Same
    /// `Unavailability` and the same "empty axis = every value on that axis"
    /// rule as [`Person::blackouts`], so a Group away for weeks 6..14 of a Term
    /// is `{days: [], blocks: [], weeks: [6..14]}`. The app stores the
    /// complement (when the Group IS available) and inverts at assembly.
    pub blackouts: Vec<Unavailability>,
}

#[derive(Clone, Debug)]
pub struct Person {
    pub id: String,
    pub role_tags: Vec<String>,
    pub groups: Vec<GroupIdx>,
    /// Windows in which this Person is unavailable. Enforced for Sessions they
    /// LEAD, by `LecturerVeto` — never for Sessions they merely attend.
    pub blackouts: Vec<Unavailability>,
    /// Days and blocks this Person would RATHER have, priced by
    /// `PersonPreferenceFit`. Counted for Sessions they LEAD, like `blackouts`
    /// above and for the same reason.
    ///
    /// Note the INVERTED emptiness relative to `blackouts`: `None` here means
    /// "no preference", where an empty axis on an `Unavailability` means "every
    /// value on that axis". See [`crate::preferences::Preference`].
    pub preferred: Option<Preference>,
    /// Rooms this Person may teach in — HARD, and enforced only for Sessions
    /// they LEAD, exactly like `blackouts` above.
    ///
    /// A WHITELIST, so empty means "any Room the Offering itself allows",
    /// which is every Person before this field existed. That emptiness is
    /// inverted relative to every mask in this module, where empty means
    /// "nothing", so `Problem::build` stores the COMPLEMENT once — see
    /// [`Problem::room_is_barred`]. Doing it anywhere else puts the trap
    /// (empty = everything, or empty = nothing?) in the hot path.
    ///
    /// Enablement is `ConstraintSet::lecturer_room_pin`, not this field: the
    /// values are Person data, the switch is tenant policy, and the split is
    /// `LecturerVeto`'s.
    pub allowed_rooms: Vec<RoomIdx>,
}

/// A blackout window.
///
/// An empty list on an axis means "every value on that axis", so `{days:[5]}`
/// is every Friday and `{blocks:[0]}` is every first block. All three empty
/// therefore means always unavailable, which is the literal reading and is
/// preserved rather than silently treated as "never".
///
/// The axes are a CROSS PRODUCT — they intersect, they do not union — and a
/// blackout LIST is the union of its windows. Those two facts together make
/// the format fully expressive over `(week, day, block)`: `{days:[3,4,5],
/// weeks:[5]}` is Wednesday to Friday of week 5 and of no other week, and an
/// absence crossing a week boundary is two windows. A date-precise fourth axis
/// has been proposed (Calendry #118) and is not needed; the over-block that
/// motivated it is the caller rounding a partial week up before assembly. See
/// `crates/core/tests/mid_week_absence.rs`, which pins this.
#[derive(Clone, Debug, Default)]
pub struct Unavailability {
    pub days: Vec<u32>,
    pub blocks: Vec<u32>,
    pub weeks: Vec<u32>,
}

impl Unavailability {
    #[inline]
    pub fn matches(&self, f: &crate::slots::SlotFlags) -> bool {
        (self.days.is_empty() || self.days.contains(&f.iso_weekday))
            && (self.blocks.is_empty() || self.blocks.contains(&f.block))
            && (self.weeks.is_empty() || self.weeks.contains(&f.week))
    }
}

/// Why a piece of occupancy cannot be moved.
///
/// Recording *why* rather than just *that* is what makes the deferred v2
/// minimize-movement policy a policy change instead of a rewrite: v2 relaxes
/// exactly one of these variants and no others.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Immovable {
    /// Explicit user lock. Absolute — never relaxed, not even in v2.
    Locked,
    /// Starts before the caller's `reference_slot`. Absolute, correctness rule.
    Past,
    /// Outside the requested scope. The ONLY variant v2 may relax.
    OutOfScope,
    /// Another tenant's use of a Federation-shared Room.
    External,
}

/// Decide whether an existing Session may be moved by this run, and if not, why.
///
/// Precedence is deliberate. `Past` is checked first because past exclusion is
/// unconditional and independent of user intent — a past Session is excluded
/// whether or not anyone locked it. `Locked` outranks `OutOfScope` because a
/// lock is absolute in every version, whereas being out of scope is merely
/// expensive to violate and is exactly what the deferred v2 policy relaxes.
pub fn classify_immovable(
    start: SlotIdx,
    reference: Option<SlotIdx>,
    is_locked: bool,
    in_scope: bool,
) -> Option<Immovable> {
    let is_past = match reference {
        Some(r) => start < r,
        None => true,
    };
    if is_past {
        return Some(Immovable::Past);
    }
    if is_locked {
        return Some(Immovable::Locked);
    }
    if !in_scope {
        return Some(Immovable::OutOfScope);
    }
    None
}

// ---------------------------------------------------------------------------
// Constraint configuration
// ---------------------------------------------------------------------------

/// One configured instance of a constraint type.
///
/// A type can be configured more than once with different `kinds`, which is why
/// this is a list rather than a single optional id.
#[derive(Clone, Debug)]
pub struct ConstraintInstance {
    pub id: String,
    /// Tenant-defined Session/Offering kinds this instance covers.
    /// **Empty means all kinds.**
    pub kinds: Vec<String>,
}

impl ConstraintInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

// ---------------------------------------------------------------------------
// Offering relations (ADR-0028)
// ---------------------------------------------------------------------------

/// A relation's evaluator. Every variant reads the same `members` list;
/// only what "satisfied" means differs — see each variant's own doc.
///
/// Deliberately its own enum, not folded into `ConstraintInstance`/kind
/// scoping: a relation names specific Offerings, never a category, so
/// `ConstraintInstance::covers` (kind-scoped) has nothing to say about it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RelationKind {
    /// HARD. No two members may ever share a slot — enforced as occupancy,
    /// the same shape as `RoomDoubleBooking`, just keyed by relation instead
    /// of Room. Symmetric: member order is irrelevant.
    DifferentTime,
    /// HARD, but PRICED at `hard_penalty` rather than enforced as an
    /// occupancy filter (unlike `DifferentTime`) — see the module doc on
    /// [`crate::constraints::same_relations`] for why a live
    /// filter cannot check SET equality against a still-incomplete week.
    /// For each week where 2+ members have a placed Session, those members'
    /// SETS of `(day, block)` pairs used that week must be exactly equal —
    /// the strict reading, combining `SameDays` and `SameStart`. Per-week,
    /// best-effort: a week where fewer than 2 members have a placed Session
    /// imposes no constraint, so this never requires members to share
    /// `required_session_count`.
    SameTime,
    /// HARD, same pricing stance as `SameTime`. For each week where 2+
    /// members have a placed Session, those members' SETS of days used
    /// that week must be exactly equal — blocks may differ.
    SameDays,
    /// HARD, same pricing stance as `SameTime`. For each week where 2+
    /// members have a placed Session, those members' SETS of start blocks
    /// used that week must be exactly equal — days may differ.
    SameStart,
    /// HARD, but enforced the OPPOSITE way `SameTime`/`SameDays`/`SameStart`
    /// are: a true occupancy FILTER, not a priced rescan. "Can Share Room,
    /// Same Room, Same Time and Same Days" at once — whichever member is
    /// placed first in a given week establishes that week's (start, room);
    /// every other member's Session that week is then restricted to EXACTLY
    /// that cell, and the two (or more) members occupy the exclusive Room
    /// TOGETHER rather than clashing on it, subject to their SUMMED
    /// `min_capacity` against `Room.capacity`. Unlike the three PRICED
    /// kinds, a full week SET is not what is being compared — this is a
    /// single, exact (start, room) match, which IS checkable as a candidate
    /// filter (see `crate::solution::Occupancy`'s `meet_together_anchor`/
    /// `meet_together_cells`), the same reason `DifferentTime` can be one.
    /// Scoped to the PRIMARY Room only; `additional_rooms` are never shared.
    MeetTogether,
    /// HARD, same "priced, not filtered" stance as `SameTime` — and the ONLY
    /// kind that reads `members`' ORDER, which is why ADR-0028 kept the set
    /// ordered. The members form a CHAIN: the rule binds each consecutive
    /// pair `(members[i], members[i+1])`, and every placed Session of the
    /// predecessor must end before every placed Session of the successor
    /// begins.
    ///
    /// "All pairs ordered" reduces to one comparison — the predecessor's
    /// LATEST end against the successor's EARLIEST start — so each
    /// consecutive pair has exactly ONE boundary, and both parameters are
    /// measured across it. See
    /// [`crate::constraints::precedence_violations`] for the arithmetic and
    /// why this cannot be an occupancy filter.
    ///
    /// Best-effort on the unplaced side: a member with no placed Session
    /// imposes nothing on the boundaries it participates in.
    Precedence {
        /// Wall-clock minutes required at the boundary, resolved through
        /// [`crate::slots::GridTime`]. 0 = back-to-back is fine, but the
        /// ordering itself still holds.
        min_gap_minutes: u32,
        /// FLOOR on the boundary in CALENDAR days, the counterpart to
        /// [`Self::Precedence::max_days_between`]'s ceiling. 0 = no floor
        /// beyond the ordering itself.
        ///
        /// The "N days between" family, as a scalar rather than a
        /// `NextDay`/`TwoDaysAfter` kind: those would be two hardcoded
        /// values of one parameter (ADR-0024), and welding `1` and `2` into
        /// type names is the magic-number shape this catalogue replaced.
        ///
        /// NOT expressible through `min_gap_minutes`, which is why it
        /// exists — see `constraints::for_each_precedence_breach` and
        /// ADR-0028's day-floor addendum for the impossibility argument.
        min_days_between: u32,
        /// Ceiling on the boundary in CALENDAR days (not teaching days).
        /// 0 = unbounded.
        max_days_between: u32,
    },
}

/// One configured Offering relation — an ordered set of Offering references
/// plus a type, per ADR-0028. `members` is ordered because `Precedence` reads
/// the order; every other kind ignores it. (A `Next Day` kind was once
/// expected to be the second such reader. It is not coming: the day-counted
/// family is `Precedence`'s `min_days_between` parameter — ADR-0028's day-floor
/// addendum.)
#[derive(Clone, Debug)]
pub struct RelationSpec {
    pub id: String,
    pub kind: RelationKind,
    pub members: Vec<OfferingIdx>,
}

/// Which constraint types are switched on, and for which kinds.
///
/// Only the types the solver actually implements are represented. Adding a type
/// here is deliberately a code change, not configuration — there is no
/// interpreter and tenant-supplied logic never executes.
#[derive(Clone, Debug, Default)]
pub struct ConstraintSet {
    pub room_double_booking: Vec<ConstraintInstance>,
    pub lecturer_double_booking: Vec<ConstraintInstance>,
    pub group_double_booking: Vec<ConstraintInstance>,
    pub person_double_booking: Vec<ConstraintInstance>,
    pub exact_frequency: Vec<ConstraintInstance>,
    /// HARD, unary: a lecturer is never assigned during their own blackout.
    /// The blackout VALUES live on `Person.blackouts`; this list only switches
    /// enforcement on.
    pub lecturer_veto: Vec<ConstraintInstance>,
    /// HARD, filterable: a lecturer pinned to a Room only leads Sessions
    /// placed there. The pin VALUES live on `Person::allowed_rooms`; this
    /// list only switches enforcement on — the same split `lecturer_veto`
    /// makes, one axis over. See [`Problem::room_is_barred`].
    pub lecturer_room_pin: Vec<ConstraintInstance>,
    /// HARD, unary: a Session is never placed during a blackout of a Group
    /// attending it. Exactly `lecturer_veto` one entity across — the windows
    /// live on `Group.blackouts`, this list only switches enforcement on.
    pub group_veto: Vec<ConstraintInstance>,
    /// SOFT, day-granularity. Was a HARD feasibility filter until the
    /// reclassification: the tenant asked for "no switching between online and
    /// in-person for a group on one day" to be a preference the solver may
    /// break when it has no better option, rather than a rule that eliminates
    /// the placement outright.
    ///
    /// Its own list rather than `soft` below, because `SoftModel` is a
    /// precomputed `(slot, room)` table and a mixed day depends on what else is
    /// already placed — see [`crate::aggregates::DayMixInstance`].
    pub online_onsite_same_day: Vec<DayMixInstance>,
    /// HARD, aggregate ratio. Carried on the objective rather than as a filter
    /// — see [`crate::aggregates`].
    pub max_online_share: Vec<ShareInstance>,
    /// SOFT, per-placement. Rewards placements landing in the days and blocks
    /// this placement's lecturers stated they would rather have.
    ///
    /// Its own list rather than `soft` below, for the same reason
    /// `online_onsite_same_day` has one: `SoftModel` is a precomputed
    /// `(profile, slot, room)` table and a preference cost is not keyed that
    /// way — it depends on who leads the placement. See
    /// [`crate::preferences`], which also explains why the COST is nevertheless
    /// part of `Objective::soft` where the day-mix cost is not.
    pub person_preference_fit: Vec<PreferenceInstance>,
    /// The six soft types. Separate from the hard lists because only soft
    /// instances carry a weight and typed parameters.
    pub soft: Vec<SoftInstance>,
    /// SOFT, aggregate over a whole day. Idle blocks between a Group's or
    /// Person's first and last Session of the day — closest in shape to
    /// `online_onsite_same_day` (day granularity, depends on what else is
    /// placed that day) but accumulates a continuous count rather than a
    /// boolean, so it gets its own counters in [`crate::aggregates::Aggregates`]
    /// rather than reusing the mixed-day ones. See
    /// [`crate::aggregates::CompactnessInstance`].
    pub compactness: Vec<CompactnessInstance>,
    /// SOFT, aggregate over an Offering's placed Sessions. Prices an Offering
    /// tagged [`SchedulingPattern::Distributed`] for spreading across more
    /// than one weekly slot. See
    /// [`crate::aggregates::PatternAdherenceInstance`] and
    /// `BlockPatternAdherence` for the other pattern's counterpart.
    pub distributed_pattern_adherence: Vec<PatternAdherenceInstance>,
    /// SOFT, aggregate over an Offering's placed Sessions across the WHOLE
    /// TERM. Prices an Offering tagged [`SchedulingPattern::Block`] for
    /// spreading across a wide span of weeks — the `Compactness` gap shape, at
    /// week granularity and scoped by Offering.
    pub block_pattern_adherence: Vec<PatternAdherenceInstance>,
    /// HARD, validation-shaped. Cross-checks a placement's Room capacity
    /// against its Offering's own Groups' summed `Group.size` — see
    /// [`crate::constraints::group_size_fits_room`].
    pub group_size_fits_room: Vec<ConstraintInstance>,
    /// HARD, filterable. Caps concurrent online Sessions tenant-wide,
    /// independent of kind — see [`Problem::max_concurrent_online`], the
    /// derived scalar the search actually enforces.
    pub max_concurrent_online_sessions: Vec<MaxConcurrentOnlineInstance>,
    /// SOFT, per-`(offering, room)`. Rewards a good Room-size fit — see
    /// [`Problem::capacity_waste_cost`].
    pub minimize_capacity_waste: Vec<CapacityWasteInstance>,
    /// SOFT, per-placement: discourage occupying a `Room::is_specialized`
    /// Room with teaching that requires none of its features. See
    /// [`Offering::specialized_room_charge`], where the whole decision is
    /// precomputed.
    pub minimize_specialized_room_use: Vec<MinimizeSpecializedRoomUseInstance>,
    /// SOFT, per-placement: discourage a Session from sitting in an EXAM week
    /// — or, with `invert`, from sitting outside one.
    ///
    /// NOT a [`crate::soft::SoftParams`] variant, and deliberately: since an
    /// exam week may be scoped to Groups ([`ProblemSpec::exam_week_groups`]),
    /// the predicate reads which cohorts attend THIS Offering, and the soft
    /// table is keyed by a kind-profile that cannot express that. Same move
    /// ADR-0026 made for `PersonPreferenceFit`, for the same reason. See
    /// [`Problem::exam_week_cost`] and [`Offering::exam_week_slots`], where
    /// the whole decision is precomputed. ADR-0033.
    pub minimize_exam_week: Vec<MinimizeExamWeekInstance>,
    /// HARD, filterable. A tenant-wide reserved window — see
    /// [`Offering::protected_block_slots`], the precomputed mask.
    pub protected_block: Vec<ProtectedBlockInstance>,
    /// SOFT, aggregate over a whole day. The mirror image of `compactness`:
    /// caps how many blocks in a row a Group or Person may be scheduled
    /// without a break, rather than minimizing the gaps between Sessions.
    /// See [`crate::aggregates::MaxConsecutiveInstance`].
    pub max_consecutive_blocks: Vec<MaxConsecutiveInstance>,
    /// SOFT, aggregate over a whole day. Caps the elapsed time from a
    /// Group's or Person's first to last Session of a day — distinct from
    /// both `compactness` (gaps inside the span) and `max_consecutive_blocks`
    /// (density): a day can have zero gaps and low density and still run too
    /// long if the bracketing Sessions are simply far apart. See
    /// [`crate::aggregates::MaxDailySpanInstance`].
    pub max_daily_span: Vec<MaxDailySpanInstance>,
    /// SOFT, aggregate over a day. Caps a raw Session COUNT per day for a
    /// Group and/or a Person — the volume-limit sibling of `max_daily_span`
    /// (elapsed time) and `max_consecutive_blocks` (continuity): a day can
    /// satisfy both of those and still be overloaded, e.g. 6 lessons split
    /// 3 + gap + 3. See [`crate::aggregates::MaxDailySessionCountInstance`].
    pub max_daily_session_count: Vec<MaxDailySessionCountInstance>,
    /// SOFT, aggregate over `(Offering, day)`. Caps how many blocks of ONE
    /// Offering may run back to back in a day — distinguishes an intentional
    /// multi-block Session (`Offering.duration_blocks`) from several
    /// separate Sessions of the same Offering landing consecutively by
    /// accident. See [`crate::aggregates::MaxConsecutiveOfferingBlocksInstance`].
    pub max_consecutive_offering_blocks: Vec<MaxConsecutiveOfferingBlocksInstance>,
    /// SOFT, aggregate over `(Offering, day)`. Caps a raw Session COUNT of
    /// ONE Offering on one day — "Maths, 4x a week" means four different
    /// days unless a tenant says otherwise. See
    /// [`crate::aggregates::MaxOfferingSessionsPerDayInstance`].
    pub max_offering_sessions_per_day: Vec<MaxOfferingSessionsPerDayInstance>,
    /// SOFT, aggregate over `(Offering, day)`. Prices the number of
    /// non-contiguous runs of one Offering's Sessions within a day, minus
    /// one — NOT the same question `compactness` asks: a day packed solid
    /// with unrelated teaching in between two runs of the same Offering has
    /// zero gaps and still splits it. See
    /// [`crate::aggregates::MinimizeOfferingDaySplitInstance`].
    pub minimize_offering_day_split: Vec<MinimizeOfferingDaySplitInstance>,
    /// SOFT, aggregate over a week. Caps how many Sessions (or blocks) a
    /// lecturer teaches in one week. See
    /// [`crate::aggregates::MaxWeeklyTeachingLoadInstance`].
    pub max_weekly_teaching_load: Vec<MaxWeeklyTeachingLoadInstance>,
    /// SOFT, aggregate over a day. A Group should not sit two or more
    /// exam-kind Sessions on the same day. See
    /// [`crate::aggregates::ExamSpacingSameDayInstance`].
    pub exam_spacing_same_day: Vec<ExamSpacingSameDayInstance>,
    /// SOFT, aggregate over a day. The generalized sibling of
    /// `exam_spacing_same_day`: a minimum day count between any two
    /// exam-kind Sessions of one Group. See
    /// [`crate::aggregates::ExamSpacingWindowInstance`].
    pub exam_spacing_window: Vec<ExamSpacingWindowInstance>,
    /// SOFT, aggregate over a week. Spreads a Group's Sessions evenly across
    /// its active days, rather than clustering on some and leaving others
    /// empty. See [`crate::aggregates::MinimizeWeekdayImbalanceInstance`].
    pub minimize_weekday_imbalance: Vec<MinimizeWeekdayImbalanceInstance>,
    /// SOFT, aggregate over a day. Penalizes a Group's or Person's day for
    /// touching more than a configured number of distinct `Room.location`
    /// values — reduces cross-campus walking between back-to-back Sessions.
    /// See [`crate::aggregates::MinimizeLocationChangeInstance`].
    pub minimize_location_change: Vec<MinimizeLocationChangeInstance>,
    /// SOFT, pairwise like the four structural double-booking types but keyed
    /// by a configurable BUFFER DISTANCE rather than exact-slot overlap.
    /// Requires a minimum gap between two bookings of the same Room. See
    /// [`crate::aggregates::RoomTurnaroundBufferInstance`].
    pub room_turnaround_buffer: Vec<RoomTurnaroundBufferInstance>,
    /// SOFT, week-granularity aggregate over a Group. Caps how many distinct
    /// Rooms a Group uses across a whole week — the "home room" concept.
    /// Distinct from `minimize_location_change`: this counts distinct ROOMS
    /// over a WEEK, not distinct LOCATIONS within one day. See
    /// [`crate::aggregates::MinimizeRoomChurnInstance`].
    pub minimize_room_churn: Vec<MinimizeRoomChurnInstance>,
    /// SOFT, aggregate over an entire Offering's Sessions across the WHOLE
    /// TERM — keyed by Offering rather than Group, unbounded by day or
    /// window, the same shape `lecturer_consistency` uses for the lecturer
    /// axis. See [`crate::aggregates::RoomConsistencyInstance`].
    pub room_consistency: Vec<RoomConsistencyInstance>,
    /// SOFT, the lecturer-axis counterpart of `room_consistency`: once a
    /// lecturer holds one Session of a recurring Offering, they should hold
    /// the rest of it too. Only ever priced for an Offering with a genuine
    /// lecturer pool (`Offering::has_lecturer_pool`) — a fixed assignment's
    /// distinct lecturer count never changes. See
    /// [`crate::aggregates::LecturerConsistencyInstance`].
    pub lecturer_consistency: Vec<LecturerConsistencyInstance>,
    /// SOFT, per-placement exact delta. Discourages a Session's span from
    /// crossing a `GridTime` gap — depends only on this placement's own day,
    /// start block and duration, so it needs no state beyond `GridTime`
    /// itself. See [`Problem::break_spanning_cost`].
    pub minimize_break_spanning: Vec<MinimizeBreakSpanningInstance>,
    /// HARD. Caps the number of distinct days a Group's or Person's
    /// Sessions may use, per week — priced at `hard_penalty` rather than a
    /// construction filter (ADR-0025). See
    /// [`crate::aggregates::MaxDaysInstance`].
    pub max_days: Vec<MaxDaysInstance>,
    /// HARD, the consecutive-run counterpart of `max_days`. See
    /// [`MaxConsecutiveDaysInstance`].
    pub max_consecutive_days: Vec<MaxConsecutiveDaysInstance>,
    /// SOFT. Requires minimum wall-clock rest between a Group's or Person's
    /// last occupied block of one teaching day and their first of the next.
    /// See [`DaybreakInstance`].
    pub daybreak: Vec<DaybreakInstance>,
    /// SOFT. Requires a minimum gap when a Group's or Person's consecutive
    /// same-day placements are in Rooms whose `Room.location` differs. See
    /// [`TravelTimeInstance`].
    pub travel_time_between_rooms: Vec<TravelTimeInstance>,
    /// SOFT, aggregate over an Offering's placed Sessions across the WHOLE
    /// TERM. Prices an Offering with `Offering.prefer_fuller_days` set for
    /// spreading across more than one distinct day — independent of
    /// `scheduling_pattern` (WHEN in the term). Same shape as
    /// `distributed_pattern_adherence`/`block_pattern_adherence`
    /// (`PatternAdherenceInstance` has no params beyond id/kinds/weight),
    /// reduced by DAY instead of weekly cell or week.
    pub minimize_offering_distinct_days: Vec<PatternAdherenceInstance>,
}

/// One `ProtectedBlock` instance. The FIRST hard type whose values
/// (`windows`) are pure tenant policy carried directly on the constraint
/// config, rather than living on a Person/Group and merely switched on here.
#[derive(Clone, Debug)]
pub struct ProtectedBlockInstance {
    pub id: String,
    pub kinds: Vec<String>,
    /// Reuses [`Unavailability`]'s day/block/week vocabulary and "empty axis
    /// = every value on that axis" convention — a recurring-weekly block is
    /// `weeks: []`, a one-off is a specific `weeks` list, exactly like a
    /// Group's own blackouts.
    pub windows: Vec<Unavailability>,
}

impl ProtectedBlockInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One `MinimizeCapacityWaste` instance. Not a `SoftParams` variant: unlike
/// every table-based soft type, its cost depends on THIS Offering's own
/// `min_capacity`, not only on `(kind-profile, slot, room)` — the same
/// reason `PersonPreferenceFit` lives outside `SoftModel` (ADR-0026), though
/// this needs neither slot nor day/block, so [`Problem::capacity_waste_cost`]
/// is a plain formula rather than its own precomputed model.
#[derive(Clone, Debug)]
pub struct CapacityWasteInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
    pub waste_ratio_threshold: f64,
}

/// One `MinimizeBreakSpanning` instance. No parameters beyond id/kinds/
/// weight — the cost depends only on the placement's own start block and
/// duration against `Problem::grid_time`, already on the wire. Kept outside
/// `SoftModel` for the same reason `CapacityWasteInstance` is: this needs no
/// `(kind-profile, slot, room)` table at all, just [`Problem::grid_time`]
/// and the placement's own span, so [`Problem::break_spanning_cost`] is a
/// plain formula.
#[derive(Clone, Debug)]
pub struct MinimizeBreakSpanningInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
}

/// One `MinimizeSpecializedRoomUse` instance. Same id/kinds/weight shape as
/// [`MinimizeBreakSpanningInstance`], and outside `SoftModel` for a sharper
/// version of the same reason: its cost depends on whether THIS Offering
/// requires any of the Room's features, and two Offerings of one kind — one
/// profile, one table row — routinely differ there. A `(kind-profile, slot,
/// room)` table cannot express it at all.
#[derive(Clone, Debug)]
pub struct MinimizeSpecializedRoomUseInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
}

impl MinimizeSpecializedRoomUseInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One `MinimizeExamWeek` instance. Same id/kinds/weight shape as
/// [`MinimizeSpecializedRoomUseInstance`], and outside `SoftModel` for the
/// same class of reason: its cost depends on which Groups this Offering
/// serves, and two Offerings of one kind — one profile, one table row —
/// routinely serve different cohorts.
///
/// `invert` selects the direction, mirroring `MinimizeRoomRank`:
///   false — penalize placing IN this Offering's exam period: keep ordinary
///           lessons out of it.
///   true  — penalize placing OUTSIDE it, pushing exam-kind Sessions toward
///           the exam period instead of away from it. Scoping this to
///           exam-kind Sessions is the tenant's job via `kinds`; the type has
///           no notion of kind itself.
///
/// A flag rather than two types, for ADR-0024's reason — but note that the
/// flag does NOT make the two directions mutually exclusive: a tenant may
/// instantiate both, which is why an Offering carries a separate charge for
/// each side of its mask.
#[derive(Clone, Debug)]
pub struct MinimizeExamWeekInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
    pub invert: bool,
}

impl MinimizeExamWeekInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

impl MinimizeBreakSpanningInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

impl CapacityWasteInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One `MaxConcurrentOnlineSessions` instance. Not a plain
/// [`ConstraintInstance`]: the cap value is a message field
/// (`max_concurrent`), not `ConstraintConfig.weight`, and this type does not
/// read `kinds` at all — every online Session counts regardless of kind, by
/// design.
#[derive(Clone, Debug)]
pub struct MaxConcurrentOnlineInstance {
    pub id: String,
    pub max_concurrent: u32,
}

fn any_covers(list: &[ConstraintInstance], kind: &str) -> bool {
    list.iter().any(|c| c.covers(kind))
}

/// Which structural checks the constructive heuristic should avoid violating
/// for a given kind.
///
/// This is an approximation of the authoritative pairwise rule, and knowingly a
/// conservative one: a violation requires *one instance covering both* sessions'
/// kinds, whereas this asks whether *some* instance covers this kind. The two
/// differ only when a type is configured twice with disjoint kind sets, in which
/// case the heuristic merely avoids a placement it did not strictly need to.
/// Being conservative in the heuristic is safe; the evaluator remains exact.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Enforce {
    pub room: bool,
    pub lecturer: bool,
    pub group: bool,
    pub person: bool,
    pub lecturer_veto: bool,
    pub lecturer_room_pin: bool,
    pub group_veto: bool,
    pub day_mix: bool,
    pub compactness_group: bool,
    pub compactness_person: bool,
    pub distributed_pattern: bool,
    pub block_pattern: bool,
    pub protected_block: bool,
    pub max_consecutive_group: bool,
    pub max_consecutive_person: bool,
    pub max_daily_span_group: bool,
    pub max_daily_span_person: bool,
    pub max_daily_session_count_group: bool,
    pub max_daily_session_count_person: bool,
    pub max_consecutive_offering_blocks: bool,
    pub max_offering_sessions_per_day: bool,
    pub minimize_offering_day_split: bool,
    pub max_weekly_teaching_load: bool,
    pub exam_spacing_same_day: bool,
    pub exam_spacing_window: bool,
    pub minimize_weekday_imbalance: bool,
    pub minimize_location_change_group: bool,
    pub minimize_location_change_person: bool,
    pub room_turnaround: bool,
    pub minimize_room_churn: bool,
    pub room_consistency: bool,
    pub lecturer_consistency: bool,
    pub max_days_group: bool,
    pub max_days_person: bool,
    pub max_consecutive_days_group: bool,
    pub max_consecutive_days_person: bool,
    pub daybreak_group: bool,
    pub daybreak_person: bool,
    pub travel_group: bool,
    pub travel_person: bool,
    pub minimize_offering_distinct_days: bool,
}

impl ConstraintSet {
    pub fn enforce_for_kind(&self, kind: &str) -> Enforce {
        Enforce {
            room: any_covers(&self.room_double_booking, kind),
            lecturer: any_covers(&self.lecturer_double_booking, kind),
            group: any_covers(&self.group_double_booking, kind),
            person: any_covers(&self.person_double_booking, kind),
            lecturer_veto: any_covers(&self.lecturer_veto, kind),
            lecturer_room_pin: any_covers(&self.lecturer_room_pin, kind),
            group_veto: any_covers(&self.group_veto, kind),
            // Own predicate: `DayMixInstance` is not a `ConstraintInstance`
            // any more, since it carries a weight.
            day_mix: self.online_onsite_same_day.iter().any(|c| c.covers(kind)),
            compactness_group: self.compactness.iter().any(|c| c.group && c.covers(kind)),
            compactness_person: self.compactness.iter().any(|c| c.person && c.covers(kind)),
            distributed_pattern: self
                .distributed_pattern_adherence
                .iter()
                .any(|c| c.covers(kind)),
            block_pattern: self.block_pattern_adherence.iter().any(|c| c.covers(kind)),
            protected_block: self.protected_block.iter().any(|c| c.covers(kind)),
            max_consecutive_group: self
                .max_consecutive_blocks
                .iter()
                .any(|c| c.group && c.covers(kind)),
            max_consecutive_person: self
                .max_consecutive_blocks
                .iter()
                .any(|c| c.person && c.covers(kind)),
            max_daily_span_group: self
                .max_daily_span
                .iter()
                .any(|c| c.group && c.covers(kind)),
            max_daily_span_person: self
                .max_daily_span
                .iter()
                .any(|c| c.person && c.covers(kind)),
            max_daily_session_count_group: self
                .max_daily_session_count
                .iter()
                .any(|c| c.group && c.covers(kind)),
            max_daily_session_count_person: self
                .max_daily_session_count
                .iter()
                .any(|c| c.person && c.covers(kind)),
            max_consecutive_offering_blocks: self
                .max_consecutive_offering_blocks
                .iter()
                .any(|c| c.covers(kind)),
            max_offering_sessions_per_day: self
                .max_offering_sessions_per_day
                .iter()
                .any(|c| c.covers(kind)),
            minimize_offering_day_split: self
                .minimize_offering_day_split
                .iter()
                .any(|c| c.covers(kind)),
            max_weekly_teaching_load: self.max_weekly_teaching_load.iter().any(|c| c.covers(kind)),
            exam_spacing_same_day: self.exam_spacing_same_day.iter().any(|c| c.covers(kind)),
            exam_spacing_window: self.exam_spacing_window.iter().any(|c| c.covers(kind)),
            minimize_weekday_imbalance: self
                .minimize_weekday_imbalance
                .iter()
                .any(|c| c.covers(kind)),
            minimize_location_change_group: self
                .minimize_location_change
                .iter()
                .any(|c| c.group && c.covers(kind)),
            minimize_location_change_person: self
                .minimize_location_change
                .iter()
                .any(|c| c.person && c.covers(kind)),
            room_turnaround: self.room_turnaround_buffer.iter().any(|c| c.covers(kind)),
            minimize_room_churn: self.minimize_room_churn.iter().any(|c| c.covers(kind)),
            room_consistency: self.room_consistency.iter().any(|c| c.covers(kind)),
            lecturer_consistency: self.lecturer_consistency.iter().any(|c| c.covers(kind)),
            max_days_group: self.max_days.iter().any(|c| c.group && c.covers(kind)),
            max_days_person: self.max_days.iter().any(|c| c.person && c.covers(kind)),
            max_consecutive_days_group: self
                .max_consecutive_days
                .iter()
                .any(|c| c.group && c.covers(kind)),
            max_consecutive_days_person: self
                .max_consecutive_days
                .iter()
                .any(|c| c.person && c.covers(kind)),
            daybreak_group: self.daybreak.iter().any(|c| c.group && c.covers(kind)),
            daybreak_person: self.daybreak.iter().any(|c| c.person && c.covers(kind)),
            travel_group: self
                .travel_time_between_rooms
                .iter()
                .any(|c| c.group && c.covers(kind)),
            travel_person: self
                .travel_time_between_rooms
                .iter()
                .any(|c| c.person && c.covers(kind)),
            minimize_offering_distinct_days: self
                .minimize_offering_distinct_days
                .iter()
                .any(|c| c.covers(kind)),
        }
    }
}

// ---------------------------------------------------------------------------
// Input specs -> derived problem
// ---------------------------------------------------------------------------

/// How an Offering's demand should distribute across the Term. See
/// `DistributedPatternAdherence`/`BlockPatternAdherence` in
/// `crate::aggregates` for the two constraint types that read this.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SchedulingPattern {
    /// Untouched by either enforcing constraint type — today's implicit
    /// behavior, unchanged.
    #[default]
    Unspecified,
    /// A consistent weekly slot for the whole Term.
    Distributed,
    /// The whole demand concentrated into a short contiguous window.
    Block,
}

/// An Offering as supplied, before closures are derived.
#[derive(Clone, Debug)]
pub struct OfferingSpec {
    pub id: String,
    pub kind: String,
    pub required_session_count: u32,
    pub duration_blocks: u32,
    /// The Offering's ASSIGNED lecturers when there is no genuine pool —
    /// `candidate_lecturer_ids.len() <= required_lecturer_count` on the wire.
    /// Empty when `eligible_lecturer_combinations` is non-empty instead: a
    /// pool Offering has no single fixed assignment, so the two are never
    /// both populated for the same Offering.
    pub lecturers: Vec<PersonIdx>,
    /// Every valid combination of `required_lecturer_count` distinct
    /// candidates from the wire's `candidate_lecturer_ids`, precomputed by
    /// the caller (`convert::build_offerings`) — combinatorics is a
    /// wire-shape concern, not a domain one, mirroring
    /// `eligible_room_combinations`. Empty and unread unless the Offering
    /// genuinely has a pool (`Offering::has_lecturer_pool`) — unlike Rooms,
    /// core keeps no separate "required count" field for this fork, because
    /// nothing filters a lecturer combination out the way capacity filters a
    /// Room one: whenever conversion populates this list at all, it is
    /// non-empty, so its emptiness alone is a reliable signal.
    pub eligible_lecturer_combinations: Vec<[Option<PersonIdx>; MAX_LECTURERS]>,
    pub groups: Vec<GroupIdx>,
    pub participants: Vec<PersonIdx>,
    pub eligible_rooms: Vec<RoomIdx>,
    /// How many Rooms one Session of this Offering must occupy
    /// simultaneously. `0` and `1` both mean today's single-Room behavior —
    /// `eligible_rooms` alone decides eligibility, `eligible_room_combinations`
    /// is never consulted.
    pub required_room_count: u32,
    /// Every valid combination of `required_room_count` distinct Rooms whose
    /// SUMMED capacity meets `min_capacity`, precomputed by the caller
    /// (`convert::build_offerings`) — combinatorics is a wire-shape concern,
    /// not a domain one, so core only ever reads this list. Empty and unread
    /// unless `required_room_count > 1`.
    pub eligible_room_combinations: Vec<(RoomIdx, [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS])>,
    pub scheduling_pattern: SchedulingPattern,
    /// Independent of `scheduling_pattern` (WHEN in the term): prefer this
    /// Offering's Sessions land on fewer distinct days across the term
    /// (fuller days) rather than one per day. See
    /// `MinimizeOfferingDistinctDays`.
    pub prefer_fuller_days: bool,
    /// The tenant-declared minimum, already spent as a HARD eligibility
    /// filter in `convert::build_offerings` (`eligible_rooms`/
    /// `eligible_room_combinations` never contain a Room too small for it).
    /// Kept here too, unlike before, because `MinimizeCapacityWaste` needs
    /// the RAW number to grade how much LARGER the assigned Room is — a
    /// question the eligibility filter's boolean pass/fail already answered
    /// and discarded. `0` means no requirement was ever stated.
    pub min_capacity: u32,
    /// Every feature name this Offering requires of its Room, from BOTH wire
    /// lists (`required_room_features` and `room_feature_requirements`).
    ///
    /// Kept for the same reason `min_capacity` above is: room ELIGIBILITY is
    /// already resolved into `eligible_rooms` by the caller, so this is not a
    /// second filter — but `MinimizeSpecializedRoomUse` needs to know not
    /// just WHICH Rooms are eligible but WHY, since a specialized Room is
    /// exempt from its charge exactly when the Offering requires something
    /// that Room provides. Empty for the overwhelming majority of Offerings,
    /// which require nothing.
    pub required_room_features: Vec<String>,
}

/// Immovable occupancy as supplied, before closures are derived.
#[derive(Clone, Debug)]
pub struct FixedSpec {
    pub session_id: String,
    /// Occupancy from another tenant's use of a Federation-shared Room —
    /// never a Session of this snapshot's tenant. The output's
    /// `retained_session_ids` accounting (ADR-0032) excludes it: only a
    /// Session the caller actually sent can be "retained".
    pub external: bool,
    /// The Offering this Session realizes, when it realizes one.
    ///
    /// `None` for ad-hoc Sessions (a `staff_meeting` kind need not realize any
    /// Offering) and for external Federation occupancy, which belongs to another
    /// tenant entirely.
    ///
    /// Load-bearing, not decorative: without it a locked Session cannot be
    /// counted toward its Offering's `ExactFrequency`, and a mid-term re-solve
    /// would schedule the full required count *on top of* the Sessions that
    /// already exist.
    pub offering: Option<OfferingIdx>,
    pub kind: String,
    pub room: Option<RoomIdx>,
    /// Additional Rooms this (already-placed) Session occupies beyond `room`
    /// — see [`crate::solution::Placement::additional_rooms`]. `[None; MAX_ADDITIONAL_ROOMS]`
    /// unless the Session realizes a `required_room_count > 1` Offering.
    pub additional_rooms: [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS],
    pub start: SlotIdx,
    pub duration_blocks: u32,
    pub lecturers: Vec<PersonIdx>,
    pub groups: Vec<GroupIdx>,
    pub persons: Vec<PersonIdx>,
    pub reason: Immovable,
}

#[derive(Clone, Debug)]
pub struct Offering {
    pub id: String,
    pub kind: String,
    pub required_session_count: u32,
    pub duration_blocks: u32,
    /// See [`OfferingSpec::lecturers`].
    pub lecturers: Vec<PersonIdx>,
    /// See [`OfferingSpec::eligible_lecturer_combinations`].
    pub eligible_lecturer_combinations: Vec<[Option<PersonIdx>; MAX_LECTURERS]>,
    /// The Offering's own Groups, unexpanded. Used to **query** occupancy.
    pub own_groups: Vec<GroupIdx>,
    /// `own_groups` expanded through ancestors and descendants. Used to **mark**
    /// occupancy. See [`crate::groups`] for why only one side expands.
    pub conflict_groups: Vec<GroupIdx>,
    /// Directly-assigned individuals, independent of Group membership. Kept
    /// distinct from `attendees` because output must report who was assigned
    /// individually, not everyone who happens to be in the room.
    pub participants: Vec<PersonIdx>,
    /// Everyone in the room: direct participants plus members of the Groups and
    /// their descendants. Attendance propagates downward only.
    pub attendees: Vec<PersonIdx>,
    pub eligible_rooms: Vec<RoomIdx>,
    /// See [`OfferingSpec::required_room_count`].
    pub required_room_count: u32,
    /// See [`OfferingSpec::eligible_room_combinations`].
    pub eligible_room_combinations: Vec<(RoomIdx, [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS])>,
    /// See [`OfferingSpec::min_capacity`].
    pub min_capacity: u32,
    pub enforce: Enforce,
    /// Index into the soft cost tables for this Offering's `kind`.
    pub soft_profile: usize,
    /// Slots blocked by the blackouts of this Offering's lecturers. Unary, so
    /// it precomputes into a mask exactly like the soft costs do.
    pub veto_slots: BitSet,
    /// Slots blocked by the blackouts of this Offering's own Groups and their
    /// ANCESTORS. Separate mask from `veto_slots` rather than one union,
    /// because the two are separately enableable (`Enforce`) and a violation
    /// has to name which entity was unavailable.
    pub group_veto_slots: BitSet,
    /// Slots blocked by a tenant-wide `ProtectedBlock` window covering this
    /// Offering's `kind` — the first hard mask whose values are pure
    /// constraint-config policy rather than per-entity data. Union of every
    /// enabled instance's windows; empty when none covers this `kind`.
    pub protected_block_slots: BitSet,
    /// `own_groups` expanded DOWNWARD only. Used by the two Group-scoped
    /// aggregate types, matching attendance semantics: a cohort Session is
    /// attended by its classes, but a class Session does not implicate the
    /// cohort.
    pub subtree_groups: Vec<GroupIdx>,
    pub scheduling_pattern: SchedulingPattern,
    /// See `OfferingSpec::prefer_fuller_days`.
    pub prefer_fuller_days: bool,
    /// What one placement of this Offering costs for occupying a SPECIALIZED
    /// Room it does not need — the summed weight of every
    /// `MinimizeSpecializedRoomUse` instance covering this Offering's kind,
    /// or `0.0` when none does.
    ///
    /// Precomputed alongside [`Self::charged_specialized_rooms`] so the hot
    /// path is a bit test and a float read, never a `Vec<String>`
    /// intersection: the exemption is a feature-set question, and answering
    /// it inside `score_one` would put string comparison in the innermost
    /// loop the search has.
    pub specialized_room_charge: f64,
    /// Which Rooms actually cost this Offering
    /// [`Self::specialized_room_charge`] — every `Room::is_specialized` Room
    /// whose `features` this Offering requires NONE of. Empty when no
    /// instance covers this kind, so the charge is unreachable rather than
    /// merely zero.
    pub charged_specialized_rooms: BitSet,
    /// Which slots fall in an EXAM week for THIS Offering's cohorts — every
    /// slot whose week is an exam week either term-globally or for a Group in
    /// `{own_groups} ∪ ancestors(own_groups)`.
    ///
    /// `expand_ancestry`, for ADR-0027's reason one axis over: an exam period
    /// declared on a programme covers its cohorts, so the QUERY walks up. An
    /// Offering serving cohorts whose periods differ gets their UNION, which
    /// is the correct answer for both directions of the flag — see ADR-0033.
    ///
    /// Unlike [`Self::charged_specialized_rooms`] this is NOT emptied when the
    /// charges are zero. An empty mask does not mean "free": under `invert` it
    /// means *charged at every slot*, so emptying it as a shortcut would make
    /// the inverted direction silently cost nothing.
    pub exam_week_slots: BitSet,
    /// What one placement of this Offering costs for sitting INSIDE
    /// [`Self::exam_week_slots`] — the summed weight of every
    /// `MinimizeExamWeek` instance covering this kind with `invert: false`.
    pub exam_week_charge_in: f64,
    /// What one placement costs for sitting OUTSIDE
    /// [`Self::exam_week_slots`] — the same sum over instances with
    /// `invert: true`. Separate from [`Self::exam_week_charge_in`] because a
    /// tenant may enable both directions at once, and one Offering must then
    /// be able to be charged on either side of its own mask.
    pub exam_week_charge_out: f64,
    /// Dense row indices into the problem's `DifferentTime` relations this
    /// Offering is a member of — empty for every Offering not named in one,
    /// which is the overwhelming majority. Precomputed the same way
    /// `veto_slots` is: derived once in `Problem::build`, read every time a
    /// Session of this Offering is marked, unmarked or probed for a slot.
    pub different_time_relations: Vec<u32>,
    /// Dense row indices into the problem's `MeetTogether` relations this
    /// Offering is a member of — parallel to `different_time_relations`,
    /// separately tracked because the two mechanisms behind them differ
    /// entirely (a shared block bit vs. an anchored, capacity-checked Room
    /// share). Empty for every Offering not named in one.
    pub meet_together_relations: Vec<u32>,
}

impl Offering {
    /// Whether this Offering's Sessions must occupy more than one Room at
    /// once — the single fork point construction and repair branch on, and
    /// the room-choice methods below key off the same way.
    #[inline]
    pub fn multi_room(&self) -> bool {
        self.required_room_count > 1
    }

    /// How many room choices exist to iterate — `eligible_room_combinations`
    /// for a multi-Room Offering, `eligible_rooms` otherwise. The "room
    /// dimension" width of the `starts x rooms` candidate cross product.
    pub fn room_choice_count(&self) -> usize {
        if self.multi_room() {
            self.eligible_room_combinations.len()
        } else {
            self.eligible_rooms.len()
        }
    }

    /// The `i`th room choice, as `(primary, additional)` — always the shape
    /// [`crate::solution::Placement::with_rooms`] wants, so callers never
    /// branch on `multi_room` themselves.
    pub fn room_choice(&self, i: usize) -> (RoomIdx, [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS]) {
        if self.multi_room() {
            self.eligible_room_combinations[i]
        } else {
            (self.eligible_rooms[i], [None; MAX_ADDITIONAL_ROOMS])
        }
    }

    /// Whether `(room, additional_rooms)` is one of this Offering's valid
    /// room choices — the one place a candidate move's Room set is checked
    /// against eligibility, so construction's greedy scan and repair's batch
    /// scoring can never silently disagree on what counts as eligible.
    pub fn is_room_choice_eligible(
        &self,
        room: RoomIdx,
        additional_rooms: [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS],
    ) -> bool {
        if self.multi_room() {
            self.eligible_room_combinations
                .iter()
                .any(|&(r, a)| r == room && a == additional_rooms)
        } else {
            additional_rooms == [None; MAX_ADDITIONAL_ROOMS] && self.eligible_rooms.contains(&room)
        }
    }

    /// Whether this Offering's Sessions choose their lecturers from a
    /// genuine candidate pool, rather than a fixed pre-assigned set — the
    /// single fork point construction and repair branch on for the
    /// lecturer dimension, the same way `multi_room` forks the Room one.
    #[inline]
    pub fn has_lecturer_pool(&self) -> bool {
        !self.eligible_lecturer_combinations.is_empty()
    }

    /// How many lecturer choices exist to iterate — `1` for a fixed
    /// assignment (there is nothing to choose between), or
    /// `eligible_lecturer_combinations.len()` for a genuine pool. Never `0`:
    /// an Offering with a pool but no valid combination is a conversion-time
    /// refusal (`ConvertError::TooManyLecturersRequired`), not a runtime
    /// state this has to represent.
    pub fn lecturer_choice_count(&self) -> usize {
        if self.has_lecturer_pool() {
            self.eligible_lecturer_combinations.len()
        } else {
            1
        }
    }

    /// The `i`th lecturer choice. For a fixed assignment this is always
    /// `[None; MAX_LECTURERS]` — a sentinel meaning "use `lecturers`, not a
    /// chosen combination" — the same reading [`crate::solution::Placement::
    /// lecturers`] gives it.
    pub fn lecturer_choice(&self, i: usize) -> [Option<PersonIdx>; MAX_LECTURERS] {
        if self.has_lecturer_pool() {
            self.eligible_lecturer_combinations[i]
        } else {
            [None; MAX_LECTURERS]
        }
    }

    /// Whether `lecturers` is one of this Offering's valid lecturer choices
    /// — mirrors [`Self::is_room_choice_eligible`], the one place a
    /// candidate move's lecturer choice is checked against eligibility.
    pub fn is_lecturer_choice_eligible(
        &self,
        lecturers: [Option<PersonIdx>; MAX_LECTURERS],
    ) -> bool {
        if self.has_lecturer_pool() {
            self.eligible_lecturer_combinations.contains(&lecturers)
        } else {
            lecturers == [None; MAX_LECTURERS]
        }
    }

    /// How many lecturers a genuine pool combination fills — for
    /// `LecturerConsistency`, which prices a pool Offering's distinct-lecturer
    /// count against this. Only meaningful when [`Self::has_lecturer_pool`]:
    /// every combination has the same number of `Some` entries, so the first
    /// one answers it. Callers must check `has_lecturer_pool` themselves —
    /// this returns `0` rather than panicking for a fixed assignment, which
    /// is never a value `LecturerConsistency` reads.
    pub fn lecturer_required_count(&self) -> u32 {
        self.eligible_lecturer_combinations
            .first()
            .map_or(0, |combo| combo.iter().filter(|l| l.is_some()).count() as u32)
    }
}

#[derive(Clone, Debug)]
pub struct FixedOccupancy {
    pub session_id: String,
    /// See [`FixedSpec::external`].
    pub external: bool,
    /// See [`FixedSpec::offering`].
    pub offering: Option<OfferingIdx>,
    pub kind: String,
    pub room: Option<RoomIdx>,
    /// See [`FixedSpec::additional_rooms`].
    pub additional_rooms: [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS],
    pub start: SlotIdx,
    pub duration_blocks: u32,
    pub lecturers: Vec<PersonIdx>,
    pub own_groups: Vec<GroupIdx>,
    pub conflict_groups: Vec<GroupIdx>,
    pub attendees: Vec<PersonIdx>,
    pub reason: Immovable,
    pub enforce: Enforce,
    pub subtree_groups: Vec<GroupIdx>,
    /// Resolved from `offering`'s own `scheduling_pattern`, so a locked or
    /// past Session of a patterned Offering still counts toward its
    /// distinct-slot/idle-week tracking. `Unspecified` — inert — for an
    /// ad-hoc Session realizing no Offering.
    pub scheduling_pattern: SchedulingPattern,
    /// Resolved from `offering`'s own `different_time_relations` — a locked
    /// Session of a related Offering still occupies the relation's shared
    /// slot, exactly like any of its other occupancy axes. Empty for an
    /// ad-hoc Session realizing no Offering.
    pub different_time_relations: Vec<u32>,
    /// Resolved from `offering`'s own `meet_together_relations` — a locked
    /// Session of a related Offering still counts as holding (or joining)
    /// the relation's shared Room, exactly like `different_time_relations`
    /// above. Empty for an ad-hoc Session realizing no Offering.
    pub meet_together_relations: Vec<u32>,
    /// Resolved from `offering`'s own `min_capacity`, needed only when this
    /// Session is ALSO a `meet_together_relations` member — its own share of
    /// the combined-capacity sum a shared Room is checked against. `0`
    /// (never charged) for an ad-hoc Session realizing no Offering.
    pub min_capacity: u32,
}

/// Which Offerings this run is actively placing.
///
/// Real membership, declared by the caller. It is deliberately *not* inferred
/// from whether an Offering owns placement variables: that inference is lossy in
/// exactly one direction, and the direction that matters. Deducting
/// already-locked Sessions can drive an in-scope Offering's placement count to
/// zero, at which point an **over-supplied** Offering (more locks than it
/// requires) looks identical to an out-of-scope one and its frequency mismatch
/// goes unreported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ScopeSpec {
    /// Every Offering in the snapshot is in scope.
    ///
    /// The correct default for the hand-written fixtures and for generated
    /// benchmark instances: both build the whole instance from nothing, so there
    /// is no out-of-scope region for a lock policy to protect.
    #[default]
    All,
    /// Exactly these Offerings are in scope. Everything else is immovable.
    Offerings(Vec<OfferingIdx>),
}

/// Everything [`Problem::build`] needs, as named fields.
///
/// This replaces eight positional arguments. Three independent call sites
/// assemble a `Problem` — the service's conversion boundary, the hand-written
/// fixtures, and the benchmark generator — and every one of them passed `vec![]`
/// fillers to satisfy the positional shape. That shape is how `scope` came to be
/// dropped: the boundary resolved it, used it twice, and had nowhere to put it.
///
/// Construct with [`ProblemSpec::new`] and override the fields you care about:
///
/// ```
/// # use calendry_solver_core::problem::{ConstraintSet, ProblemSpec, Problem};
/// # use calendry_solver_core::slots::{SlotTable, WeekKind, WeekSpec};
/// # let slots = SlotTable::build(2, &[1], &[WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] }]).unwrap();
/// let problem = Problem::build(ProblemSpec {
///     constraints: ConstraintSet::default(),
///     ..ProblemSpec::new(slots)
/// })
/// .unwrap();
/// ```
#[derive(Clone, Debug)]
pub struct ProblemSpec {
    /// The tenant's grid, flattened. There is no default: every arithmetic
    /// question about time resolves against this, so a caller must supply one.
    pub slots: SlotTable,
    pub rooms: Vec<Room>,
    pub groups: Vec<Group>,
    pub persons: Vec<Person>,
    pub offerings: Vec<OfferingSpec>,
    pub placements: Vec<PlacementVar>,
    pub fixed: Vec<FixedSpec>,
    pub constraints: ConstraintSet,
    /// Rules relating specific Offerings to each other, per ADR-0028 — never
    /// scoped by kind, so kept independent of `constraints` above.
    pub relations: Vec<RelationSpec>,
    pub scope: ScopeSpec,
    /// Bias against disturbing a movable out-of-scope placement, under
    /// `LOCK_POLICY_MINIMIZE_MOVEMENT`. Meaningless — and left at its default
    /// of `0.0` — when nothing in `placements` carries an `original`, exactly
    /// like every other soft weight with nothing to weigh.
    pub movement_weight: f64,
    /// The in-scope counterpart of `movement_weight`: bias against disturbing
    /// an in-scope placement that already realizes an existing Session, away
    /// from where it already sat. A SEPARATE weight rather than reusing
    /// `movement_weight` — issue #58 ("In-scope Sessions have no stay-put
    /// pressure") — because the two conflate different magnitudes: "do not
    /// disturb the neighbours" (out-of-scope) and "do not churn what a
    /// targeted repair was not asked to touch" (in-scope) are different
    /// products sharing one mechanism. See [`Problem::movement_cost`].
    pub in_scope_movement_weight: f64,
    /// Per-entity exceptions to the two weights above — see
    /// [`MovementOverrides`]. Empty leaves both behaving exactly as they did
    /// before this existed.
    pub movement_overrides: MovementOverrides,
    /// The grid's wall-clock gap structure — see [`GridTime`]. Defaults to
    /// no gaps anywhere, which is inert unless `MinimizeBreakSpanning` or
    /// `Daybreak` is configured.
    pub grid_time: GridTime,
    /// The run's "now", as a slot: no NEW placement may start before it
    /// (ADR-0032). `None` — the default, and what every fixture and the
    /// benchmark generator use — means no reference exists and nothing is
    /// masked. The conversion layer, whose wire semantics define "no
    /// reference" as "the reference lies beyond the term", maps that case to
    /// one-past-the-last-slot instead, masking everything.
    pub reference: Option<SlotIdx>,
    /// Which Groups each EXAM week is an exam week FOR (ADR-0033). A week
    /// absent from this list — or present with an empty `groups` — is an exam
    /// week for EVERY Group, which is what every exam week was before this
    /// existed and what the wire's absent `Week.exam_group_ids` decodes to.
    ///
    /// Lands on the spec rather than on [`crate::slots::WeekSpec`] for the
    /// reason the whole design rests on: `slots` is deliberately the
    /// group-free coordinate system, `week_kind` stays a property of a slot
    /// rather than of a `(slot, Group)` pair, and the `SlotTable` is built
    /// before the `GroupIdx` space exists at all. Same place
    /// `movement_overrides` lands, for the same reason.
    ///
    /// Several entries for one week are a UNION — the natural spelling of "A
    /// and B both sit exams in week 12" — and the wire cannot produce them,
    /// since it carries one list per `Week`.
    pub exam_week_groups: Vec<ExamWeekScope>,
}

/// Which Groups one EXAM week belongs to (ADR-0033).
///
/// An id may name a Group at any level and binds that Group's DESCENDANTS: a
/// programme's exam fortnight covers its cohorts. The query side therefore
/// walks UP through [`crate::groups::GroupClosure::expand_ancestry`] — the
/// same downward-binding direction `GroupVeto` uses (ADR-0027), and
/// deliberately not the both-directions propagation double-booking uses.
///
/// `groups` empty means every Group, so this type can only ever NARROW.
#[derive(Clone, Debug)]
pub struct ExamWeekScope {
    pub week: u32,
    pub groups: Vec<GroupIdx>,
}

/// Which Persons' and Groups' Sessions carry their OWN movement weight
/// instead of the run-wide one (issue #70).
///
/// A repair-mode selector: some people and cohorts are fine to move, others
/// should be disturbed only if nothing else resolves the repair. Both are one
/// number — `0.0` is "movable, no extra cost" even under a large run-wide
/// weight, and a large value is soft-unmovable. It stays SOFT: unlike a
/// Session lock, an override can never prevent a move, only price it.
///
/// **A `persons` entry covers Sessions that Person LECTURES**, never ones
/// they merely attend — the scope decision ADR-0026 records for
/// `PersonPreferenceFit`, for the same reason (an attendee set averages ~65
/// people at benchmark scale, so an attendee reading would leave nearly every
/// Session overridden). A student's protection goes through their Group.
///
/// **A `groups` entry binds that Group and its DESCENDANTS**, so a Session
/// attached to `g` is covered by an entry on `g` or on any ancestor of `g` —
/// resolved through [`crate::groups::GroupClosure::expand_ancestry`], the
/// same downward-binding direction `GroupVeto` uses and deliberately NOT the
/// both-directions propagation double-booking uses (ADR-0027).
#[derive(Clone, Debug, Default)]
pub struct MovementOverrides {
    pub persons: Vec<(PersonIdx, f64)>,
    pub groups: Vec<(GroupIdx, f64)>,
}

impl MovementOverrides {
    pub fn is_empty(&self) -> bool {
        self.persons.is_empty() && self.groups.is_empty()
    }
}

impl ProblemSpec {
    /// An empty instance on `slots`, with every Offering in scope.
    pub fn new(slots: SlotTable) -> Self {
        Self {
            slots,
            rooms: Vec::new(),
            groups: Vec::new(),
            persons: Vec::new(),
            offerings: Vec::new(),
            placements: Vec::new(),
            fixed: Vec::new(),
            constraints: ConstraintSet::default(),
            relations: Vec::new(),
            scope: ScopeSpec::All,
            movement_weight: 0.0,
            in_scope_movement_weight: 0.0,
            movement_overrides: MovementOverrides::default(),
            grid_time: GridTime::default(),
            reference: None,
            exam_week_groups: Vec::new(),
        }
    }

    /// Expand each Offering's required count into placement variables, deducting
    /// what immovable Sessions already realize.
    ///
    /// This is the **degenerate** expansion: no Session-id reuse, no
    /// occupancy-aware placement of the locks themselves. The conversion
    /// boundary and the benchmark generator each need more than this and keep
    /// their own expansion; what they share is the arithmetic, which
    /// [`Problem::residual_for`] now states once so all three can be checked
    /// against it.
    ///
    /// `saturating_sub`, never `-`: the caller's editing UX is "warn and allow",
    /// so more Sessions than an Offering requires is legitimate input, and
    /// wrapping a `u32` would ask the solver to place four billion Sessions.
    pub fn expand_placements(&mut self) -> &mut Self {
        let mut realized = vec![0u32; self.offerings.len()];
        for f in &self.fixed {
            if let Some(o) = f.offering
                && o.get() < realized.len()
            {
                realized[o.get()] += 1;
            }
        }

        self.placements.clear();
        for (i, o) in self.offerings.iter().enumerate() {
            let outstanding = o.required_session_count.saturating_sub(realized[i]);
            for occurrence in 0..outstanding {
                self.placements.push(PlacementVar {
                    offering: OfferingIdx(i as u32),
                    occurrence,
                    existing_session_id: None,
                    original: None,
                });
            }
        }
        self
    }
}

/// Incremental assembly that hands back the index of everything it inserts.
///
/// The hazard it removes: `Group.parent`, `Person.groups` and
/// `OfferingSpec.groups` are raw indices into vectors the caller has not
/// finished building, and a stale one is not a reportable error — `build`
/// declares only [`GroupCycle`], so a dangling index panics on a raw slice index
/// deep inside closure derivation. Returning the typed index *from the insert*
/// makes the wrong value unavailable rather than merely discouraged.
///
/// ```
/// # use calendry_solver_core::ids::PersonIdx;
/// # use calendry_solver_core::problem::{Group, Person, ProblemBuilder};
/// # use calendry_solver_core::slots::{SlotTable, WeekKind, WeekSpec};
/// # let slots = SlotTable::build(2, &[1], &[WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] }]).unwrap();
/// let mut b = ProblemBuilder::new(slots);
/// let cohort = b.group(Group { id: "C".into(), parent: None, name: "C".into(), size: 0, blackouts: vec![] });
/// let class = b.group(Group { id: "K".into(), parent: Some(cohort), name: "K".into(), size: 0, blackouts: vec![] });
/// let _student = b.person(Person {
///     id: "s".into(),
///     role_tags: vec![],
///     groups: vec![class],
///     blackouts: vec![],
///     preferred: None,
///     allowed_rooms: vec![],
/// });
/// let problem = b.build().unwrap();
/// ```
pub struct ProblemBuilder {
    spec: ProblemSpec,
}

impl ProblemBuilder {
    /// An empty instance on `slots`, with every Offering in scope.
    pub fn new(slots: SlotTable) -> Self {
        Self { spec: ProblemSpec::new(slots) }
    }

    /// Wrap an existing spec, so a caller can mix bulk field assignment with
    /// index-returning inserts.
    pub fn from_spec(spec: ProblemSpec) -> Self {
        Self { spec }
    }

    pub fn room(&mut self, room: Room) -> RoomIdx {
        self.spec.rooms.push(room);
        RoomIdx(self.spec.rooms.len() as u32 - 1)
    }

    /// Insert a Group. Its `parent`, if any, is already a `GroupIdx` this
    /// builder handed out, so it cannot dangle.
    pub fn group(&mut self, group: Group) -> GroupIdx {
        self.spec.groups.push(group);
        GroupIdx(self.spec.groups.len() as u32 - 1)
    }

    pub fn person(&mut self, person: Person) -> PersonIdx {
        self.spec.persons.push(person);
        PersonIdx(self.spec.persons.len() as u32 - 1)
    }

    pub fn offering(&mut self, offering: OfferingSpec) -> OfferingIdx {
        self.spec.offerings.push(offering);
        OfferingIdx(self.spec.offerings.len() as u32 - 1)
    }

    pub fn fixed(&mut self, fixed: FixedSpec) -> &mut Self {
        self.spec.fixed.push(fixed);
        self
    }

    pub fn constraints(&mut self, constraints: ConstraintSet) -> &mut Self {
        self.spec.constraints = constraints;
        self
    }

    pub fn scope(&mut self, scope: ScopeSpec) -> &mut Self {
        self.spec.scope = scope;
        self
    }

    /// See [`ProblemSpec::expand_placements`].
    pub fn expand_placements(&mut self) -> &mut Self {
        self.spec.expand_placements();
        self
    }

    /// The spec under construction, for the fields with no dedicated setter.
    pub fn spec_mut(&mut self) -> &mut ProblemSpec {
        &mut self.spec
    }

    pub fn build(self) -> Result<Problem, GroupCycle> {
        Problem::build(self.spec)
    }
}

/// One Session that needs placing.
#[derive(Clone, Debug)]
pub struct PlacementVar {
    pub offering: OfferingIdx,
    pub occurrence: u32,
    /// Preserved when this occurrence corresponds to an existing in-scope
    /// Session, so a re-solve does not needlessly churn Session ids downstream.
    pub existing_session_id: Option<String>,
    /// Where this occurrence already sat, when it realizes an existing
    /// Session that is either out-of-scope and made movable by
    /// `LOCK_POLICY_MINIMIZE_MOVEMENT`, or in-scope and reused (see
    /// [`Problem::movement_cost`], which charges `movement_weight` for the
    /// first and `in_scope_movement_weight` for the second). `None` for a
    /// brand-new Session — nothing to be charged for leaving a place it
    /// never held.
    ///
    /// The inner `Option<RoomIdx>` mirrors [`FixedSpec::room`]: an
    /// online-only or not-yet-roomed Session is a real state there, so it
    /// must be one here too rather than being forced into a room it never had.
    pub original: Option<(SlotIdx, Option<RoomIdx>)>,
}

#[derive(Clone, Debug)]
pub struct Problem {
    pub slots: SlotTable,
    /// See [`ProblemSpec::reference`]. Enforced in
    /// [`crate::solution::SearchState::statically_blocked`], so construction,
    /// repair and the targeted ruin operator all read one definition.
    pub reference: Option<SlotIdx>,
    /// The grid's wall-clock gap structure — see [`GridTime`]. Only read by
    /// `MinimizeBreakSpanning`, `Daybreak` and `Precedence`'s
    /// `min_gap_minutes`.
    pub grid_time: GridTime,
    pub rooms: Vec<Room>,
    pub groups: Vec<Group>,
    pub persons: Vec<Person>,
    /// Per-Person room vetoes, indexed by [`PersonIdx`] — the COMPLEMENT of
    /// [`Person::allowed_rooms`], so an empty veto blocks nothing exactly as
    /// `veto_slots`, `group_veto_slots`, `protected_block_slots` and
    /// `footprint_siblings` do. The inversion happens once, in `build`, so no
    /// read site ever has to know which polarity it is holding.
    ///
    /// EMPTY when no Person states a pin, which is nearly every tenant.
    ///
    /// Read through [`Self::room_pin_blocks`] against the placement's CHOSEN
    /// lecturers, never precomputed into the Offering — see that method.
    person_room_veto: Vec<BitSet>,
    pub closure: GroupClosure,
    pub offerings: Vec<Offering>,
    pub placements: Vec<PlacementVar>,
    pub fixed: Vec<FixedOccupancy>,
    pub constraints: ConstraintSet,
    pub soft: SoftModel,
    /// Per-placement preference costs, precomputed. Empty and inert when no
    /// `PersonPreferenceFit` is configured.
    pub preferences: PreferenceModel,
    /// Template for the search's aggregate counters: sized and configured, but
    /// empty. The search clones it and fills it from the fixed occupancy.
    pub aggregate_template: Aggregates,
    /// Derived, never tuned: large enough that one unplaced Session outranks
    /// every reachable soft configuration, so the scalar objective orders
    /// lexicographically without a magic constant.
    pub hard_penalty: f64,
    /// Summed weight of every configured `OnlineOnsiteSameDay`, charged once
    /// per mixed `(group, day)` cell. Zero when the type is not configured, so
    /// the term costs nothing rather than needing a branch at every use.
    pub day_mix_weight: f64,
    /// HARD. The tightest `max_concurrent` among every enabled
    /// `MaxConcurrentOnlineSessions` instance — see
    /// [`crate::solution::SearchState`]'s occupancy index, which enforces it
    /// as a filter. `None` when not configured: no cap, today's behavior.
    /// Kind-independent by design: every online Session counts toward it,
    /// whatever `kind` it realizes.
    pub max_concurrent_online: Option<u32>,
    /// Bias against disturbing a movable out-of-scope placement. See
    /// [`ProblemSpec::movement_weight`] and [`Problem::movement_cost`].
    pub movement_weight: f64,
    /// See [`ProblemSpec::in_scope_movement_weight`].
    pub in_scope_movement_weight: f64,
    /// Which movement weight each Offering's placements actually pay, indexed
    /// by [`OfferingIdx`] — the two weights above already resolved against
    /// scope and against any [`MovementOverrides`] entry covering the
    /// Offering's lecturers or Groups. Precomputed because none of those
    /// inputs can change during a run, which is what keeps
    /// [`Problem::movement_cost`] a single indexed read.
    offering_movement_weight: Vec<f64>,
    /// Summed weight of every configured `Compactness` instance covering the
    /// Group axis. Zero when not configured, or when no instance selects it —
    /// see [`crate::aggregates::CompactnessInstance::group`].
    pub compactness_group_weight: f64,
    /// The Person-axis counterpart of `compactness_group_weight`.
    pub compactness_person_weight: f64,
    /// Summed weight of every configured `MaxConsecutiveBlocks` instance
    /// covering the Group axis. Zero when not configured, or when no
    /// instance selects it.
    pub max_consecutive_group_weight: f64,
    /// The Person-axis counterpart of `max_consecutive_group_weight`.
    pub max_consecutive_person_weight: f64,
    /// Summed weight of every configured `MaxDailySpan` instance covering
    /// the Group axis. Zero when not configured, or when no instance
    /// selects it.
    pub max_daily_span_group_weight: f64,
    /// The Person-axis counterpart of `max_daily_span_group_weight`.
    pub max_daily_span_person_weight: f64,
    /// Summed weight of every configured `MaxDailySessionCount` instance
    /// covering the Group axis. Zero when not configured, or when no
    /// instance selects it.
    pub max_daily_session_count_group_weight: f64,
    /// The Person-axis counterpart of `max_daily_session_count_group_weight`.
    pub max_daily_session_count_person_weight: f64,
    /// Summed weight of every configured `MaxConsecutiveOfferingBlocks`
    /// instance. Zero when not configured. Offering-keyed, no axis split.
    pub max_consecutive_offering_blocks_weight: f64,
    /// Summed weight of every configured `MaxOfferingSessionsPerDay`
    /// instance. Zero when not configured. Offering-keyed, no axis split.
    pub max_offering_sessions_per_day_weight: f64,
    /// Summed weight of every configured `MinimizeOfferingDaySplit`
    /// instance. Zero when not configured. Offering-keyed, no axis split.
    pub minimize_offering_day_split_weight: f64,
    /// Summed weight of every configured `MaxWeeklyTeachingLoad` instance.
    /// Zero when not configured. Lecturer-only, no axis split.
    pub max_weekly_teaching_load_weight: f64,
    /// Summed weight of every configured `ExamSpacingSameDay` instance.
    pub exam_same_day_weight: f64,
    /// Summed weight of every configured `ExamSpacingWindow` instance.
    pub exam_window_weight: f64,
    /// Summed weight of every configured `MinimizeWeekdayImbalance`
    /// instance.
    pub imbalance_weight: f64,
    /// Summed weight of every configured `DistributedPatternAdherence`.
    pub distributed_pattern_weight: f64,
    /// Summed weight of every configured `BlockPatternAdherence`.
    pub block_pattern_weight: f64,
    /// Summed weight of every configured `MinimizeLocationChange` instance
    /// covering the Group axis. Zero when not configured, or when no
    /// instance selects it.
    pub location_change_group_weight: f64,
    /// The Person-axis counterpart of `location_change_group_weight`.
    pub location_change_person_weight: f64,
    /// Summed weight of every configured `RoomTurnaroundBuffer` instance.
    /// Zero when not configured.
    pub room_turnaround_weight: f64,
    /// Summed weight of every configured `Daybreak` instance covering the
    /// Group axis. Zero when not configured, or when no instance selects
    /// it.
    pub daybreak_group_weight: f64,
    /// The Person-axis counterpart of `daybreak_group_weight`.
    pub daybreak_person_weight: f64,
    /// Summed weight of every configured `TravelTimeBetweenRooms` instance
    /// covering the Group axis. Zero when not configured, or when no
    /// instance selects it.
    pub travel_group_weight: f64,
    /// The Person-axis counterpart of `travel_group_weight`.
    pub travel_person_weight: f64,
    /// Summed weight of every configured `MinimizeOfferingDistinctDays`
    /// instance.
    pub minimize_offering_distinct_days_weight: f64,
    /// Summed weight of every configured `MinimizeRoomChurn` instance. Zero
    /// when not configured.
    pub room_churn_weight: f64,
    /// Summed weight of every configured `RoomConsistency` instance. Zero
    /// when not configured.
    pub room_consistency_weight: f64,
    /// Summed weight of every configured `LecturerConsistency` instance. Zero
    /// when not configured.
    pub lecturer_consistency_weight: f64,
    /// `Room.location`, interned to a dense index parallel to [`Self::rooms`].
    /// See [`Problem::room_location`].
    room_location: Vec<u32>,
    /// Every OTHER exclusive Room sharing a physical footprint with this one,
    /// parallel to [`Self::rooms`]. See [`Problem::footprint_siblings`].
    room_footprint_siblings: Vec<Vec<RoomIdx>>,
    /// Every configured `DifferentTime` relation's own id, dense, parallel to
    /// the row index every Offering's and `FixedOccupancy`'s
    /// `different_time_relations` entry names. Its length is the row count
    /// the solution module's relation occupancy matrix is sized against.
    pub different_time_relation_ids: Vec<String>,
    /// Every configured `MeetTogether` relation's own id, dense, parallel to
    /// the row index every Offering's and `FixedOccupancy`'s
    /// `meet_together_relations` entry names — the same shape
    /// `different_time_relation_ids` is, for the same reason: the occupancy
    /// index needs an O(1) row to key its anchor/cell maps by.
    pub meet_together_relation_ids: Vec<String>,
    /// Every configured `SameTime`/`SameDays`/`SameStart`/`Precedence`
    /// relation, kept whole (unlike `DifferentTime`, which is fully consumed
    /// into the per-Offering `different_time_relations` row indices at build
    /// time): these are read fresh by
    /// [`crate::constraints::same_relations`] and
    /// [`crate::constraints::precedence_relations`] rather than maintained as
    /// an occupancy bit, so the raw member list is what every rescan needs —
    /// **in its configured order**, which `Precedence` reads and no
    /// per-Offering membership row could carry. `DifferentTime` relations are
    /// never present here. `MeetTogether` relations ARE also kept here, in ADDITION to
    /// their own dense row above — the occupancy index needs the row for its
    /// hot-path anchor lookup, while `constraints::meet_together_disagreements`
    /// needs the raw member list to catch a bad LOCKED pairing the search
    /// never had a chance to avoid, the same reason `DifferentTime` has its
    /// own independent structural check despite also being occupancy-backed.
    pub relations: Vec<RelationSpec>,

    /// Whether each Offering is being actively placed by this run, indexed by
    /// [`OfferingIdx`]. Read it through [`Problem::in_scope`].
    in_scope: Vec<bool>,
    /// Placement variables owned by each Offering, precomputed.
    placement_counts: Vec<u32>,
    /// Immovable Sessions linked to each Offering, precomputed. A locked or
    /// already-past Session is still a Session that happened.
    immovable_counts: Vec<u32>,
}

impl Problem {
    /// The single derivation path, shared by the service's conversion layer, the
    /// hand-written test fixtures and the benchmark generator. Keeping one
    /// implementation is what stops them from drifting on closure semantics.
    pub fn build(spec: ProblemSpec) -> Result<Self, GroupCycle> {
        let ProblemSpec {
            slots,
            rooms,
            groups,
            persons,
            offerings,
            placements,
            fixed,
            constraints,
            relations,
            scope,
            movement_weight,
            in_scope_movement_weight,
            movement_overrides,
            grid_time,
            reference,
            exam_week_groups,
        } = spec;

        let parent_of: Vec<Option<GroupIdx>> = groups.iter().map(|g| g.parent).collect();
        let closure = GroupClosure::build(&parent_of)?;

        // Group -> direct members.
        let mut members: Vec<Vec<PersonIdx>> = vec![Vec::new(); groups.len()];
        for (i, p) in persons.iter().enumerate() {
            for g in &p.groups {
                members[g.get()].push(PersonIdx(i as u32));
            }
        }

        let attendees_of = |group_ids: &[GroupIdx], direct: &[PersonIdx]| -> Vec<PersonIdx> {
            let mut out: Vec<PersonIdx> = direct.to_vec();
            for g in closure.expand_subtree(group_ids) {
                out.extend_from_slice(&members[g.get()]);
            }
            out.sort_unstable();
            out.dedup();
            out
        };

        // Distinct kinds in play, so the soft model can build one cost table
        // per profile rather than one per kind.
        let mut kinds: Vec<String> = offerings
            .iter()
            .map(|o| o.kind.clone())
            .chain(fixed.iter().map(|f| f.kind.clone()))
            .collect();
        kinds.sort();
        kinds.dedup();

        let soft = SoftModel::build(constraints.soft.clone(), &slots, &rooms, &kinds);

        // `Room.location` interned to a dense index, first-seen order — a
        // pure function of the Room list, so any deterministic assignment is
        // fine; `MinimizeLocationChange` only cares which Rooms SHARE an
        // index, not what the index itself is.
        let mut location_index: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let room_location: Vec<u32> = rooms
            .iter()
            .map(|r| {
                let next = location_index.len() as u32;
                *location_index.entry(r.location.clone()).or_insert(next)
            })
            .collect();
        let n_locations = location_index.len();

        // Footprint closure: which OTHER exclusive Rooms a booking of each
        // Room also occupies. Resolved once, here, because none of its inputs
        // can change during a run and `Occupancy` consults it on every
        // mark/unmark/is_free — the same reason `room_location` above is
        // interned rather than compared as strings in the loop.
        //
        // Non-exclusive Rooms are dropped from BOTH sides: a virtual Room
        // neither claims a footprint nor can have one claimed against it
        // (ADR-0022), so one carrying a tag contributes nothing rather than
        // marking a row no reader consults. The wire layer refuses that
        // combination outright, so this only softens what a fixture can build.
        let mut footprint_members: std::collections::HashMap<&str, Vec<RoomIdx>> =
            std::collections::HashMap::new();
        for (i, r) in rooms.iter().enumerate() {
            if !r.is_exclusive() {
                continue;
            }
            for tag in &r.footprints {
                footprint_members
                    .entry(tag.as_str())
                    .or_default()
                    .push(RoomIdx(i as u32));
            }
        }
        let room_footprint_siblings: Vec<Vec<RoomIdx>> = rooms
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let me = RoomIdx(i as u32);
                if !r.is_exclusive() {
                    return Vec::new();
                }
                let mut out: Vec<RoomIdx> = r
                    .footprints
                    .iter()
                    .filter_map(|tag| footprint_members.get(tag.as_str()))
                    .flatten()
                    .copied()
                    .filter(|&other| other != me)
                    .collect();
                out.sort_unstable_by_key(|r| r.get());
                out.dedup();
                out
            })
            .collect();

        // Blackout -> slot mask, resolved against the tenant's grid. Unary, so
        // it precomputes once per Offering.
        let veto_mask = |lecturers: &[PersonIdx]| -> BitSet {
            let mut mask = BitSet::new(slots.len());
            for l in lecturers {
                let blackouts = &persons[l.get()].blackouts;
                if blackouts.is_empty() {
                    continue;
                }
                for slot in slots.all() {
                    let f = slots.flags(slot);
                    if blackouts.iter().any(|b| b.matches(f)) {
                        mask.insert(slot.get());
                    }
                }
            }
            mask
        };

        // The Group counterpart, walking UP from each attached Group so a
        // parent's absence reaches its children. `expand_ancestry`, not
        // `expand_subtree`: the wrong one lets one seminar veto a faculty.
        let group_veto_mask = |own: &[GroupIdx]| -> BitSet {
            let mut mask = BitSet::new(slots.len());
            for g in closure.expand_ancestry(own) {
                let blackouts = &groups[g.get()].blackouts;
                if blackouts.is_empty() {
                    continue;
                }
                for slot in slots.all() {
                    let f = slots.flags(slot);
                    if blackouts.iter().any(|b| b.matches(f)) {
                        mask.insert(slot.get());
                    }
                }
            }
            mask
        };

        // Which slots fall in an EXAM week for a given Offering's cohorts
        // (ADR-0033). `expand_ancestry` for ADR-0027's reason one axis over:
        // an exam period declared on a programme covers its cohorts, so the
        // query walks UP. An unscoped exam week matches everybody, which is
        // what every exam week was before the scope existed.
        //
        // Cohorts whose periods differ give their UNION, and that is the
        // correct answer for BOTH directions of `invert`: a joint lecture
        // collides with either cohort's exams, and a joint exam may sit in
        // either cohort's period. The intersection would leave a joint
        // Offering with no exam period at all, which under `invert` is a
        // uniform charge that steers nothing.
        let exam_week_mask = |own: &[GroupIdx]| -> BitSet {
            let mut mask = BitSet::new(slots.len());
            let ancestry = closure.expand_ancestry(own);
            for slot in slots.all() {
                let f = slots.flags(slot);
                if f.week_kind != WeekKind::Exam {
                    continue;
                }
                let mut scoped = exam_week_groups
                    .iter()
                    .filter(|s| s.week == f.week)
                    .peekable();
                let mine = scoped.peek().is_none()
                    || scoped.any(|s| {
                        s.groups.is_empty() || s.groups.iter().any(|g| ancestry.contains(g))
                    });
                if mine {
                    mask.insert(slot.get());
                }
            }
            mask
        };

        // Tenant-wide, keyed by KIND rather than by any per-entity data — the
        // first hard mask that is pure constraint-config policy. Recomputed
        // per Offering like `veto_mask`/`group_veto_mask` above rather than
        // cached per kind; the same cost those already accept.
        let protected_block_mask = |kind: &str| -> BitSet {
            let mut mask = BitSet::new(slots.len());
            for instance in &constraints.protected_block {
                if !instance.covers(kind) {
                    continue;
                }
                for window in &instance.windows {
                    for slot in slots.all() {
                        if window.matches(slots.flags(slot)) {
                            mask.insert(slot.get());
                        }
                    }
                }
            }
            mask
        };

        // `DifferentTime` relations, resolved to a dense row index (parallel
        // to `different_time_relation_ids`) and a per-Offering membership
        // list — the same "precompute once per Offering, read on every
        // mark/unmark" shape `veto_mask` above uses. `SameTime`/`SameDays`/
        // `SameStart`/`Precedence` need no such precomputation: they are read
        // fresh by `constraints::same_relations` and
        // `constraints::precedence_relations`, which want the raw member list
        // kept whole on `Problem::relations` instead — and `Precedence`
        // additionally needs it in its ORIGINAL order, which a per-Offering
        // membership row cannot express.
        //
        // `MeetTogether` needs BOTH: the dense row (its occupancy index also
        // runs a hot-path filter, like `DifferentTime`) AND the raw member
        // list (`constraints::meet_together_disagreements` re-derives a bad
        // LOCKED pairing independently, the same reason `DifferentTime` has
        // its own structural check despite being occupancy-backed too).
        let mut different_time_relation_ids: Vec<String> = Vec::new();
        let mut different_time_membership: Vec<Vec<u32>> = vec![Vec::new(); offerings.len()];
        let mut meet_together_relation_ids: Vec<String> = Vec::new();
        let mut meet_together_membership: Vec<Vec<u32>> = vec![Vec::new(); offerings.len()];
        let mut retained_relations: Vec<RelationSpec> = Vec::new();
        for r in &relations {
            match r.kind {
                RelationKind::DifferentTime => {
                    let ri = different_time_relation_ids.len() as u32;
                    different_time_relation_ids.push(r.id.clone());
                    for &o in &r.members {
                        if let Some(row) = different_time_membership.get_mut(o.get()) {
                            row.push(ri);
                        }
                    }
                }
                RelationKind::MeetTogether => {
                    let ri = meet_together_relation_ids.len() as u32;
                    meet_together_relation_ids.push(r.id.clone());
                    for &o in &r.members {
                        if let Some(row) = meet_together_membership.get_mut(o.get()) {
                            row.push(ri);
                        }
                    }
                    retained_relations.push(r.clone());
                }
                RelationKind::SameTime
                | RelationKind::SameDays
                | RelationKind::SameStart
                | RelationKind::Precedence { .. } => {
                    retained_relations.push(r.clone());
                }
            }
        }

        // `MinimizeSpecializedRoomUse`, resolved per Offering once.
        //
        // Two things collapse into a bit test here. WHICH Rooms are
        // specialized is fixed for the run, and WHETHER this Offering is
        // exempt from a given one is a feature-set intersection — a
        // `Vec<String>` scan that must never happen inside `score_one`. So
        // both are answered now, and `specialized_room_cost` becomes a bit
        // test plus a float read.
        //
        // The charge is folded in the same pass: an Offering no instance
        // covers gets an EMPTY set rather than a zero weight, so the whole
        // term is unreachable for it rather than merely free.
        let specialized_charge_of = |kind: &str| -> f64 {
            constraints
                .minimize_specialized_room_use
                .iter()
                .filter(|i| i.covers(kind))
                .map(|i| i.weight)
                .sum()
        };
        let charged_specialized_rooms_for = |kind: &str, required: &[String]| -> BitSet {
            let mut mask = BitSet::new(rooms.len());
            if specialized_charge_of(kind) == 0.0 {
                return mask;
            }
            for (i, r) in rooms.iter().enumerate() {
                // Exempt by REQUIREMENT: the class that needs the lab belongs
                // in the lab, and charging it would price a choice it never
                // had. Only teaching that could have gone elsewhere is
                // discouraged.
                if r.is_specialized && !required.iter().any(|f| r.features.contains(f)) {
                    mask.insert(i);
                }
            }
            mask
        };

        // Two charges, one per direction of `invert`, because the flag does
        // not make the directions exclusive: a tenant may enable both, and
        // one Offering must then be chargeable on either side of its mask.
        //
        // DELIBERATELY NOT following `charged_specialized_rooms_for`'s
        // empty-mask shortcut above. An empty exam-week mask does not mean
        // "free" — under `invert` it means charged at EVERY slot — so the
        // zero-charge guard belongs on the charges, in `exam_week_cost`, and
        // never on the mask.
        let exam_week_charge_of = |kind: &str, invert: bool| -> f64 {
            constraints
                .minimize_exam_week
                .iter()
                .filter(|i| i.invert == invert && i.covers(kind))
                .map(|i| i.weight)
                .sum()
        };

        let derived_offerings: Vec<Offering> = offerings
            .into_iter()
            .enumerate()
            .map(|(i, o)| Offering {
                specialized_room_charge: specialized_charge_of(&o.kind),
                charged_specialized_rooms: charged_specialized_rooms_for(
                    &o.kind,
                    &o.required_room_features,
                ),
                different_time_relations: different_time_membership[i].clone(),
                meet_together_relations: meet_together_membership[i].clone(),
                soft_profile: soft.profile_for_kind(&o.kind),
                veto_slots: veto_mask(&o.lecturers),
                group_veto_slots: group_veto_mask(&o.groups),
                // Before `o.groups` is moved into `own_groups` below, exactly
                // as `group_veto_slots` must be.
                exam_week_slots: exam_week_mask(&o.groups),
                exam_week_charge_in: exam_week_charge_of(&o.kind, false),
                exam_week_charge_out: exam_week_charge_of(&o.kind, true),
                protected_block_slots: protected_block_mask(&o.kind),
                subtree_groups: closure.expand_subtree(&o.groups),
                enforce: constraints.enforce_for_kind(&o.kind),
                conflict_groups: closure.expand_conflict(&o.groups),
                attendees: attendees_of(&o.groups, &o.participants),
                participants: o.participants,
                own_groups: o.groups,
                id: o.id,
                kind: o.kind,
                required_session_count: o.required_session_count,
                duration_blocks: o.duration_blocks,
                lecturers: o.lecturers,
                eligible_lecturer_combinations: o.eligible_lecturer_combinations,
                eligible_rooms: o.eligible_rooms,
                required_room_count: o.required_room_count,
                eligible_room_combinations: o.eligible_room_combinations,
                min_capacity: o.min_capacity,
                scheduling_pattern: o.scheduling_pattern,
                prefer_fuller_days: o.prefer_fuller_days,
            })
            .collect();

        let derived_fixed: Vec<FixedOccupancy> = fixed
            .into_iter()
            .map(|f| FixedOccupancy {
                enforce: constraints.enforce_for_kind(&f.kind),
                subtree_groups: closure.expand_subtree(&f.groups),
                conflict_groups: closure.expand_conflict(&f.groups),
                attendees: attendees_of(&f.groups, &f.persons),
                own_groups: f.groups,
                scheduling_pattern: f
                    .offering
                    .and_then(|o| derived_offerings.get(o.get()))
                    .map_or(SchedulingPattern::Unspecified, |o| o.scheduling_pattern),
                different_time_relations: f
                    .offering
                    .and_then(|o| different_time_membership.get(o.get()))
                    .cloned()
                    .unwrap_or_default(),
                meet_together_relations: f
                    .offering
                    .and_then(|o| meet_together_membership.get(o.get()))
                    .cloned()
                    .unwrap_or_default(),
                min_capacity: f
                    .offering
                    .and_then(|o| derived_offerings.get(o.get()))
                    .map_or(0, |o| o.min_capacity),
                session_id: f.session_id,
                external: f.external,
                offering: f.offering,
                kind: f.kind,
                room: f.room,
                additional_rooms: f.additional_rooms,
                start: f.start,
                duration_blocks: f.duration_blocks,
                lecturers: f.lecturers,
                reason: f.reason,
            })
            .collect();

        let weekly_cells = slots.active_days().len() * slots.blocks_per_day() as usize;

        let aggregate_template = Aggregates::new(
            groups.len(),
            slots.day_count(),
            slots.week_count() as usize,
            constraints.max_online_share.clone(),
            persons.len(),
            slots.len(),
            slots.blocks_per_day() as usize,
            constraints.compactness.clone(),
            derived_offerings.len(),
            weekly_cells,
            constraints.distributed_pattern_adherence.clone(),
            constraints.block_pattern_adherence.clone(),
            constraints.max_consecutive_blocks.clone(),
            constraints.max_daily_span.clone(),
            constraints.max_daily_session_count.clone(),
            constraints.max_offering_sessions_per_day.clone(),
            constraints.max_consecutive_offering_blocks.clone(),
            constraints.minimize_offering_day_split.clone(),
            constraints.max_weekly_teaching_load.clone(),
            constraints.exam_spacing_same_day.clone(),
            constraints.exam_spacing_window.clone(),
            constraints.minimize_weekday_imbalance.clone(),
            slots.active_days().len(),
            constraints.max_days.clone(),
            constraints.max_consecutive_days.clone(),
            constraints.minimize_location_change.clone(),
            n_locations,
            constraints.room_turnaround_buffer.clone(),
            rooms.len(),
            constraints.minimize_room_churn.clone(),
            constraints.room_consistency.clone(),
            constraints.lecturer_consistency.clone(),
            constraints.daybreak.clone(),
            constraints.travel_time_between_rooms.clone(),
            constraints.minimize_offering_distinct_days.clone(),
        );

        let day_mix_weight: f64 = constraints
            .online_onsite_same_day
            .iter()
            .map(|i| i.weight)
            .sum();

        // The TIGHTEST cap among every enabled instance — multiple caps on
        // one tenant-wide resource compose as "whichever binds hardest",
        // never as a sum. `None` (today's behavior) when nothing is
        // configured.
        let max_concurrent_online: Option<u32> = constraints
            .max_concurrent_online_sessions
            .iter()
            .map(|i| i.max_concurrent)
            .min();

        let compactness_group_weight: f64 = constraints
            .compactness
            .iter()
            .filter(|i| i.group)
            .map(|i| i.weight)
            .sum();
        let compactness_person_weight: f64 = constraints
            .compactness
            .iter()
            .filter(|i| i.person)
            .map(|i| i.weight)
            .sum();
        let max_consecutive_group_weight: f64 = constraints
            .max_consecutive_blocks
            .iter()
            .filter(|i| i.group)
            .map(|i| i.weight)
            .sum();
        let max_consecutive_person_weight: f64 = constraints
            .max_consecutive_blocks
            .iter()
            .filter(|i| i.person)
            .map(|i| i.weight)
            .sum();
        let max_daily_span_group_weight: f64 = constraints
            .max_daily_span
            .iter()
            .filter(|i| i.group)
            .map(|i| i.weight)
            .sum();
        let max_daily_span_person_weight: f64 = constraints
            .max_daily_span
            .iter()
            .filter(|i| i.person)
            .map(|i| i.weight)
            .sum();
        let max_daily_session_count_group_weight: f64 = constraints
            .max_daily_session_count
            .iter()
            .filter(|i| i.group)
            .map(|i| i.weight)
            .sum();
        let max_daily_session_count_person_weight: f64 = constraints
            .max_daily_session_count
            .iter()
            .filter(|i| i.person)
            .map(|i| i.weight)
            .sum();
        let max_consecutive_offering_blocks_weight: f64 = constraints
            .max_consecutive_offering_blocks
            .iter()
            .map(|i| i.weight)
            .sum();
        let max_offering_sessions_per_day_weight: f64 = constraints
            .max_offering_sessions_per_day
            .iter()
            .map(|i| i.weight)
            .sum();
        let minimize_offering_day_split_weight: f64 = constraints
            .minimize_offering_day_split
            .iter()
            .map(|i| i.weight)
            .sum();
        let max_weekly_teaching_load_weight: f64 = constraints
            .max_weekly_teaching_load
            .iter()
            .map(|i| i.weight)
            .sum();
        let exam_same_day_weight: f64 = constraints
            .exam_spacing_same_day
            .iter()
            .map(|i| i.weight)
            .sum();
        let exam_window_weight: f64 = constraints
            .exam_spacing_window
            .iter()
            .map(|i| i.weight)
            .sum();
        let imbalance_weight: f64 = constraints
            .minimize_weekday_imbalance
            .iter()
            .map(|i| i.weight)
            .sum();
        let location_change_group_weight: f64 = constraints
            .minimize_location_change
            .iter()
            .filter(|i| i.group)
            .map(|i| i.weight)
            .sum();
        let location_change_person_weight: f64 = constraints
            .minimize_location_change
            .iter()
            .filter(|i| i.person)
            .map(|i| i.weight)
            .sum();
        let room_turnaround_weight: f64 = constraints
            .room_turnaround_buffer
            .iter()
            .map(|i| i.weight)
            .sum();
        let daybreak_group_weight: f64 = constraints
            .daybreak
            .iter()
            .filter(|i| i.group)
            .map(|i| i.weight)
            .sum();
        let daybreak_person_weight: f64 = constraints
            .daybreak
            .iter()
            .filter(|i| i.person)
            .map(|i| i.weight)
            .sum();
        let travel_group_weight: f64 = constraints
            .travel_time_between_rooms
            .iter()
            .filter(|i| i.group)
            .map(|i| i.weight)
            .sum();
        let travel_person_weight: f64 = constraints
            .travel_time_between_rooms
            .iter()
            .filter(|i| i.person)
            .map(|i| i.weight)
            .sum();
        let minimize_offering_distinct_days_weight: f64 = constraints
            .minimize_offering_distinct_days
            .iter()
            .map(|i| i.weight)
            .sum();
        let room_churn_weight: f64 = constraints
            .minimize_room_churn
            .iter()
            .map(|i| i.weight)
            .sum();
        let room_consistency_weight: f64 =
            constraints.room_consistency.iter().map(|i| i.weight).sum();
        let lecturer_consistency_weight: f64 = constraints
            .lecturer_consistency
            .iter()
            .map(|i| i.weight)
            .sum();
        let distributed_pattern_weight: f64 = constraints
            .distributed_pattern_adherence
            .iter()
            .map(|i| i.weight)
            .sum();
        let block_pattern_weight: f64 = constraints
            .block_pattern_adherence
            .iter()
            .map(|i| i.weight)
            .sum();
        let capacity_waste_weight: f64 = constraints
            .minimize_capacity_waste
            .iter()
            .map(|i| i.weight)
            .sum();
        let specialized_room_weight: f64 = constraints
            .minimize_specialized_room_use
            .iter()
            .map(|i| i.weight)
            .sum();
        let break_spanning_weight: f64 = constraints
            .minimize_break_spanning
            .iter()
            .map(|i| i.weight)
            .sum();
        // NOT optional, and not merely tidy: `MinimizeExamWeek` used to live
        // in `ConstraintSet::soft`, so its weight used to arrive through
        // `soft.total_weight` below. Moving the type out (ADR-0033) removed
        // that contribution, and without this term `hard_penalty` would
        // silently SHRINK — letting a soft preference gain ground on a hole
        // in the timetable.
        //
        // The bound holds because one placement is charged on exactly one
        // side of its own mask: `max(charge_in, charge_out) <=
        // exam_week_weight`, so summing every instance's weight is still the
        // per-placement ceiling.
        let exam_week_weight: f64 = constraints
            .minimize_exam_week
            .iter()
            .map(|i| i.weight)
            .sum();

        // Built here rather than beside `SoftModel` above because it keys on
        // the PLACEMENT, so it needs the derived Offerings — a placement's
        // lecturer set is what it folds in.
        let preferences = PreferenceModel::build(
            constraints.person_preference_fit.clone(),
            &slots,
            &persons,
            &derived_offerings,
            &placements,
        );

        /*
         * sum(weights) * placements + 1 dominates any achievable soft total, so
         * the scalar objective orders lexicographically. Aggregate violations
         * join `unplaced` on the hard side and are covered by the same bound.
         *
         * THE DAY-MIX TERM NEEDS ITS OWN BOUND, and multiplying it by
         * `placements` would be wrong in both directions. It is charged per
         * mixed `(group, day)` CELL, and one placement can make several cells
         * mixed at once (it spans days and implicates its whole subtree of
         * Groups) while two placements are needed before any cell is mixed at
         * all. The exact ceiling is every cell being mixed, which is what the
         * counter table is sized for — so that is the multiplier, and the bound
         * stays tight rather than merely safe.
         *
         * THE PREFERENCE TERM IS PER PLACEMENT LIKE `soft`, but its weight is
         * not its ceiling: a Person may carry a bounded multiplier, so one
         * placement can cost up to `weight * MAX_WEIGHT_MULTIPLIER`. Summing
         * raw weights here would leave the bound short by exactly that factor
         * and a heavily-preferred schedule could outrank an unplaced Session.
         * `max_cost_per_placement` is computed from the constraint
         * configuration alone — not from how many lecturers have an override on
         * file — which is what keeps a tenant-editable column out of this
         * number. See [`crate::preferences::PreferenceModel`].
         *
         * THE MOVEMENT TERM IS BOUNDED LIKE `soft`: it is binary per placement
         * (moved or not), so `movement_weight` IS its own per-placement
         * ceiling, and multiplying by every placement rather than only the
         * movable ones stays safe for the same reason `soft.total_weight`
         * already does — a placement a term does not apply to costs it
         * nothing, which only widens the bound. `in_scope_movement_weight` is
         * the same shape one axis over; the two can never both charge the
         * SAME placement (a placement is in scope or is not), so summing both
         * ceilings is a safe bound, if not the tightest one. A per-entity
         * override (issue #70) REPLACES whichever of the two applies, so one
         * placement's true ceiling is `max(base, override)` — bounded by
         * adding the largest override configured, which is looser than
         * necessary and safe for the same reason the other two are.
         *
         * THE CAPACITY-WASTE TERM IS BOUNDED THE SAME WAY `MinimizeRoomRank`
         * IS: `capacity_waste_cost`'s saturating curve caps each covering
         * instance's contribution at its own `weight`, so summing every
         * instance's weight is the per-placement ceiling, same shape as
         * `soft.total_weight`.
         *
         * THE SPECIALIZED-ROOM TERM IS BOUNDED THE SAME WAY, and more
         * tightly than the others: it is FLAT and charged at most ONCE per
         * placement (a Session occupying several specialized Rooms still
         * pays its weight once), so summing every instance's weight is
         * exactly the per-placement ceiling rather than an over-estimate.
         *
         * THE BREAK-SPANNING TERM IS BOUNDED THE SAME WAY: `break_spanning_
         * cost` charges each covering instance's flat `weight` at most once
         * per placement (span crosses a gap, or it does not — no scaling by
         * how many minutes or how many gaps), so summing every instance's
         * weight is the per-placement ceiling, same shape as
         * `capacity_waste_weight`.
         */
        let max_movement_override = movement_overrides
            .persons
            .iter()
            .map(|&(_, w)| w)
            .chain(movement_overrides.groups.iter().map(|&(_, w)| w))
            .fold(0.0_f64, f64::max);
        let hard_penalty = soft.total_weight * placements.len() as f64
            + preferences.max_cost_per_placement() * placements.len() as f64
            + day_mix_weight * aggregate_template.day_mix_cell_count() as f64
            + movement_weight * placements.len() as f64
            + in_scope_movement_weight * placements.len() as f64
            + max_movement_override * placements.len() as f64
            + capacity_waste_weight * placements.len() as f64
            + specialized_room_weight * placements.len() as f64
            + break_spanning_weight * placements.len() as f64
            + exam_week_weight * placements.len() as f64
            // Read off `(group, day)` cell counts, exactly like day_mix_weight
            // above: neither is bounded by placements, since one placement can
            // touch several cells at once.
            + exam_same_day_weight * aggregate_template.exam_same_day_cell_count() as f64
            + exam_window_weight * aggregate_template.exam_window_cell_count() as f64
            + 1.0;

        let n = derived_offerings.len();
        let in_scope = match &scope {
            ScopeSpec::All => vec![true; n],
            ScopeSpec::Offerings(list) => {
                let mut flags = vec![false; n];
                for &o in list {
                    if o.get() < n {
                        flags[o.get()] = true;
                    }
                }
                flags
            }
        };

        // Which movement weight each Offering's placements pay (issue #70).
        //
        // Resolved once per Offering rather than read per move: an Offering's
        // lecturers and Groups are fixed before the search starts, and so is
        // its scope, so the answer cannot change. That is what lets
        // `movement_cost` keep its `(placement, start, room)` signature and
        // stay a single indexed read on the hottest path there is.
        //
        // A genuine lecturer POOL is the one place this is approximate, and
        // deliberately so: which candidate teaches a Session is a search-time
        // choice, so an exact answer would have to be priced per candidate —
        // the trap ADR-0026 records for the preference table. Instead an
        // override matching ANY candidate covers the Offering, which
        // over-protects rather than under-protects. That is the safe
        // direction for a soft bias: a protected person's Session stays
        // protected whichever pool member ends up teaching it.
        let offering_movement_weight: Vec<f64> = derived_offerings
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let base = if in_scope[i] { in_scope_movement_weight } else { movement_weight };
                if movement_overrides.is_empty() {
                    return base;
                }
                let lecturers = o.lecturers.iter().copied().chain(
                    o.eligible_lecturer_combinations
                        .iter()
                        .flatten()
                        .flatten()
                        .copied(),
                );
                // A Group's protection binds downward, so the QUERY walks up
                // — `expand_ancestry`, exactly as `GroupVeto` does and never
                // `expand_subtree`/`expand_conflict` (ADR-0027).
                let ancestry = closure.expand_ancestry(&o.own_groups);
                let matched = movement_overrides
                    .persons
                    .iter()
                    .filter(|(p, _)| lecturers.clone().any(|l| l == *p))
                    .map(|&(_, w)| w)
                    .chain(
                        movement_overrides
                            .groups
                            .iter()
                            .filter(|(g, _)| ancestry.contains(g))
                            .map(|&(_, w)| w),
                    )
                    // The LARGEST wins: order-independent, so it cannot
                    // depend on the order the caller sent them, and a broader
                    // "movable" never silently defeats a narrower protection.
                    .fold(f64::NEG_INFINITY, f64::max);
                if matched.is_finite() { matched } else { base }
            })
            .collect();

        let mut placement_counts = vec![0u32; n];
        for var in &placements {
            if var.offering.get() < n {
                placement_counts[var.offering.get()] += 1;
            }
        }
        let mut immovable_counts = vec![0u32; n];
        for f in &derived_fixed {
            if let Some(o) = f.offering
                && o.get() < n
            {
                immovable_counts[o.get()] += 1;
            }
        }

        // THE ONE PLACE THE WHITELIST BECOMES A BLACKLIST. `Person::
        // allowed_rooms` is a whitelist where empty means EVERY Room; every
        // mask in this module is a blacklist where empty means NOTHING. Doing
        // the inversion here, once, keeps that trap out of the hot path — and
        // out of every future reader's way, since an empty row now means what
        // an empty row means everywhere else.
        //
        // A Person with no pin gets a zero-capacity row: no allocation, and
        // `BitSet::contains` is false for every index.
        let person_room_veto: Vec<BitSet> = if persons.iter().all(|p| p.allowed_rooms.is_empty()) {
            Vec::new()
        } else {
            persons
                .iter()
                .map(|p| {
                    let mut veto = BitSet::new(rooms.len());
                    // AN EMPTY PIN VETOES NOTHING, and this early return is
                    // the whole reason the inversion lives here: the wire's
                    // whitelist has empty meaning EVERY Room, so inverting it
                    // naively would veto every Room and make one unconfigured
                    // Person unplaceable everywhere.
                    //
                    // Full width even when there is no pin, rather than a
                    // zero-capacity row: `BitSet::contains` debug-asserts its
                    // index against the capacity, so a narrow row would panic
                    // the moment anyone asked about it. The saving that
                    // matters is the `Vec::new()` above, which covers every
                    // tenant that pins nobody.
                    if p.allowed_rooms.is_empty() {
                        return veto;
                    }
                    for r in 0..rooms.len() {
                        if !p.allowed_rooms.contains(&RoomIdx(r as u32)) {
                            veto.insert(r);
                        }
                    }
                    veto
                })
                .collect()
        };

        let problem = Self {
            slots,
            reference,
            grid_time,
            rooms,
            groups,
            persons,
            person_room_veto,
            closure,
            offerings: derived_offerings,
            placements,
            fixed: derived_fixed,
            constraints,
            soft,
            preferences,
            aggregate_template,
            hard_penalty,
            day_mix_weight,
            max_concurrent_online,
            movement_weight,
            in_scope_movement_weight,
            offering_movement_weight,
            compactness_group_weight,
            compactness_person_weight,
            max_consecutive_group_weight,
            max_consecutive_person_weight,
            max_daily_span_group_weight,
            max_daily_span_person_weight,
            max_daily_session_count_group_weight,
            max_daily_session_count_person_weight,
            max_consecutive_offering_blocks_weight,
            max_offering_sessions_per_day_weight,
            minimize_offering_day_split_weight,
            max_weekly_teaching_load_weight,
            exam_same_day_weight,
            exam_window_weight,
            imbalance_weight,
            distributed_pattern_weight,
            block_pattern_weight,
            location_change_group_weight,
            location_change_person_weight,
            room_turnaround_weight,
            daybreak_group_weight,
            daybreak_person_weight,
            travel_group_weight,
            travel_person_weight,
            minimize_offering_distinct_days_weight,
            room_churn_weight,
            room_consistency_weight,
            lecturer_consistency_weight,
            room_location,
            room_footprint_siblings,
            different_time_relation_ids,
            meet_together_relation_ids,
            relations: retained_relations,
            in_scope,
            placement_counts,
            immovable_counts,
        };

        // Internal consistency, never feasibility. A producer that miscounts the
        // placement/lock split used to surface as an `ExactFrequency` violation
        // at solve time, inside a test about something else; this attributes it
        // to whichever assembly got the arithmetic wrong.
        //
        // Over-supply is deliberately NOT asserted: "warn and allow" means a
        // caller can legitimately send more Sessions than an Offering requires,
        // and rejecting that here would break the non-negotiable that the solver
        // tolerates infeasible input. Hence `> 0`, not `!= 0`.
        debug_assert!(
            problem
                .offering_ids()
                .all(|o| !problem.in_scope(o) || problem.residual_for(o) <= 0),
            "an in-scope Offering is short of placement variables against its \
             required count: {:?}",
            problem
                .offering_ids()
                .filter(|&o| problem.in_scope(o) && problem.residual_for(o) > 0)
                .map(|o| (problem.offerings[o.get()].id.clone(), problem.residual_for(o)))
                .collect::<Vec<_>>()
        );

        Ok(problem)
    }

    #[inline]
    pub fn placement_ids(&self) -> impl Iterator<Item = PlacementIdx> {
        (0..self.placements.len() as u32).map(PlacementIdx)
    }

    #[inline]
    pub fn offering_ids(&self) -> impl Iterator<Item = OfferingIdx> {
        (0..self.offerings.len() as u32).map(OfferingIdx)
    }

    /// Whether this run is actively placing `o`.
    ///
    /// Carried from the caller's scope, not inferred from placement presence.
    /// The difference is load-bearing: an Offering with more locked Sessions
    /// than it requires legitimately has **zero** placement variables, and under
    /// the old inference was indistinguishable from one nobody asked about.
    #[inline]
    pub fn in_scope(&self, o: OfferingIdx) -> bool {
        self.in_scope[o.get()]
    }

    /// Placement variables this run created for `o`.
    #[inline]
    pub fn placement_count(&self, o: OfferingIdx) -> u32 {
        self.placement_counts[o.get()]
    }

    /// Immovable Sessions that already realize `o` — locked, past, or out of
    /// scope. A locked Session is still a Session that happened, so it counts
    /// toward the Offering's required frequency.
    #[inline]
    pub fn immovable_count(&self, o: OfferingIdx) -> u32 {
        self.immovable_counts[o.get()]
    }

    /// `required_session_count` minus everything already accounted for:
    /// placement variables plus immovable realizations.
    ///
    /// Zero in a well-formed instance. **Negative means over-supplied** — more
    /// Sessions exist than the Offering claims to need, which the caller's "warn
    /// and allow" editing UX can legitimately produce and which the solver must
    /// report rather than reject. Positive means whichever assembly built the
    /// placements is short, and is asserted against in debug builds.
    #[inline]
    pub fn residual_for(&self, o: OfferingIdx) -> i64 {
        let required = i64::from(self.offerings[o.get()].required_session_count);
        required
            - i64::from(self.placement_counts[o.get()])
            - i64::from(self.immovable_counts[o.get()])
    }

    #[inline]
    pub fn placement(&self, p: PlacementIdx) -> &PlacementVar {
        &self.placements[p.get()]
    }

    #[inline]
    pub fn offering_of(&self, p: PlacementIdx) -> &Offering {
        &self.offerings[self.placements[p.get()].offering.get()]
    }

    /// What placing `p` at `(start, room)` costs for leaving where it
    /// already was.
    ///
    /// `0.0` when `p` has no `original` (a brand-new Session) or when it is
    /// placed back exactly where it was. A room-only change counts as
    /// "moved" too — this is a single knob, not a slot/room split — so the
    /// comparison is on the whole pair, not `start` alone. No table: unlike
    /// [`crate::preferences::PreferenceModel`], the cost does not depend on
    /// who leads the placement, only on where it already was, so a direct
    /// compare is most of the computation.
    ///
    /// Which weight applies is `p`'s Offering's SCOPE, not whether `original`
    /// happens to be set: `movement_weight` for an out-of-scope Session made
    /// movable by `LOCK_POLICY_MINIMIZE_MOVEMENT`, `in_scope_movement_weight`
    /// for an in-scope Session reused by a targeted repair. The two never
    /// charge the same placement, since a placement's Offering is in scope or
    /// is not — and a [`MovementOverrides`] entry covering the Offering
    /// REPLACES whichever of the two it would have been. All three are
    /// already collapsed into `offering_movement_weight` at build time, so
    /// this stays one indexed read.
    ///
    /// An override cannot make a HARD-locked Session movable: under
    /// `LOCK_POLICY_HARD` an out-of-scope Session is `FixedSpec` occupancy
    /// with no `PlacementVar` at all, so there is nothing here to charge.
    /// Movability is the lock policy's question; this only prices it.
    #[inline]
    pub fn movement_cost(&self, p: PlacementIdx, start: SlotIdx, room: RoomIdx) -> f64 {
        let var = &self.placements[p.get()];
        match var.original {
            Some(original) if original != (start, Some(room)) => {
                self.offering_movement_weight[var.offering.get()]
            }
            _ => 0.0,
        }
    }

    /// The `PersonPreferenceFit` cost of placing `p`'s Offering at `at`,
    /// branching on which of the two the Offering needs: the fast
    /// precomputed table for a fixed assignment, or the live per-person
    /// computation over `at.lecturers` for a genuine pool — see
    /// [`crate::preferences::PreferenceModel::cost_for`]'s own doc for why a
    /// pool cannot use the table. One place this branch is made, so
    /// `search::Trial::place`/`unplace` and `evaluator::score_one` cannot
    /// drift on which Offerings get which path.
    #[inline]
    pub fn preference_cost_for_placement(
        &self,
        o: &Offering,
        p: PlacementIdx,
        at: Placement,
    ) -> f64 {
        let room_features = &self.rooms[at.room.get()].features;
        if o.has_lecturer_pool() {
            self.preferences
                .cost_for(p, &at.lecturers, at.start, room_features)
        } else {
            self.preferences.cost(p, at.start, room_features)
        }
    }

    /// The capacity `capacity_waste_cost` should be charged against: the SUM
    /// of every EXCLUSIVE (non-virtual) Room in the placement, `capacity`'s
    /// same "sum across the set" convention with virtual Rooms excluded
    /// entirely.
    ///
    /// A virtual Room is not a scarce resource (ADR-0022) — it has no seats
    /// to waste, so including it here would price an online placement as
    /// though it were a lecture hall standing mostly empty. `0` for an
    /// all-virtual combination, which `capacity_waste_cost` already treats
    /// the same way it treats `min_capacity == 0`: nothing to charge.
    #[inline]
    pub fn exclusive_capacity(&self, rooms: impl Iterator<Item = RoomIdx>) -> u32 {
        rooms
            .filter(|r| !self.rooms[r.get()].is_virtual)
            .map(|r| self.rooms[r.get()].capacity)
            .sum()
    }

    /// SOFT. Rewards a good Room-size fit: charges every enabled
    /// `MinimizeCapacityWaste` instance covering `offering.kind`, scaled by
    /// how far `capacity / offering.min_capacity` exceeds the instance's
    /// `waste_ratio_threshold`. `capacity` is the caller's SUM across every
    /// EXCLUSIVE Room in a placement (see [`Self::exclusive_capacity`]) — the
    /// same "capacity is summed" convention Multi-room Sessions established
    /// for eligibility, not a per-Room ratio summed across the set, which
    /// would price the identical fit differently depending on how many Rooms
    /// happened to supply it.
    ///
    /// `min_capacity == 0` is never penalized — a ratio against zero is
    /// meaningless, and this Offering never asked for a minimum at all.
    ///
    /// WHY A SATURATING CURVE, not a raw ratio multiplier: `hard_penalty`
    /// relies on every soft-side term being bounded by ITS OWN weight per
    /// placement (see the derivation above), and a raw ratio has no natural
    /// ceiling the way `MinimizeRoomRank` has the room set's own rank span
    /// to normalize against — a 500-seat hall for a 1-person tutorial would
    /// otherwise blow the bound open. `excess / (excess + 1)` approaches 1.0
    /// as the ratio grows without needing a configured "worst case".
    #[inline]
    pub fn capacity_waste_cost(&self, offering: &Offering, capacity: u32) -> f64 {
        if offering.min_capacity == 0 {
            return 0.0;
        }
        let ratio = capacity as f64 / offering.min_capacity as f64;
        self.constraints
            .minimize_capacity_waste
            .iter()
            .filter(|i| i.covers(&offering.kind))
            .map(|i| {
                let excess = (ratio - i.waste_ratio_threshold).max(0.0);
                i.weight * (excess / (excess + 1.0))
            })
            .sum()
    }

    /// What placing `offering` into `rooms` costs for occupying a SPECIALIZED
    /// Room it does not need — a lab, computer room or workshop that should
    /// have stayed free for teaching that requires it.
    ///
    /// FLAT and charged at most ONCE per placement, however many specialized
    /// Rooms it occupies. `Room::is_specialized` is a boolean, so unlike
    /// `MinimizeCapacityWaste`'s ratio or `MinimizeRoomRank`'s distance past a
    /// threshold there is no gradient to grade — and charging per Room would
    /// make one placement's ceiling `weight * MAX_ROOMS_PER_SESSION`, which
    /// `hard_penalty` would then have to widen for a case that barely exists.
    ///
    /// Every decision is already precomputed into
    /// [`Offering::charged_specialized_rooms`] — which Rooms are specialized,
    /// whether this Offering is exempt from each, and whether any instance
    /// covers its kind at all — so this stays a bit test on the hot path.
    #[inline]
    pub fn specialized_room_cost(
        &self,
        offering: &Offering,
        rooms: impl Iterator<Item = RoomIdx>,
    ) -> f64 {
        if offering.charged_specialized_rooms.is_empty() {
            return 0.0;
        }
        let charged = rooms
            .into_iter()
            .any(|r| offering.charged_specialized_rooms.contains(r.get()));
        if charged { offering.specialized_room_charge } else { 0.0 }
    }

    /// Is any of `lecturers` pinned away from any of `rooms`? HARD — see
    /// [`Person::allowed_rooms`] and [`ConstraintSet::lecturer_room_pin`].
    ///
    /// `false` outright when no Person carries a pin, which is the common
    /// case and costs one `is_empty`.
    ///
    /// TAKES THE CANDIDATE'S LECTURERS, not an Offering. Where an Offering has
    /// a genuine lecturer pool the lecturer set is chosen during the search,
    /// so an Offering-level mask could only hold the union of its candidates'
    /// pins (permissive — it admits a Room no eventual lecturer may use) or
    /// their intersection (restrictive — it bars a Room the chosen lecturer
    /// may use). `LecturerVeto` holds exactly such a mask, which is why
    /// `LecturerVeto` plus a pool has to be refused at conversion; asking the
    /// question against the chosen set instead is what makes a pool the case
    /// this rule SERVES rather than the case it breaks.
    ///
    /// EVERY Room must satisfy EVERY pinned lecturer: a pin that only one Room
    /// had to satisfy could be escaped by requiring more Rooms.
    ///
    /// The single predicate behind both the search filter
    /// ([`crate::solution::SearchState::statically_blocked`]) and the
    /// authoritative report ([`crate::constraints::lecturer_room_pin`]), so
    /// the solver cannot refuse a placement it then declines to report
    /// (ADR-0014, ADR-0022).
    #[inline]
    pub fn room_pin_blocks(
        &self,
        lecturers: impl Iterator<Item = PersonIdx>,
        rooms: impl Iterator<Item = RoomIdx>,
    ) -> bool {
        if self.person_room_veto.is_empty() {
            return false;
        }
        // Collected onto the stack rather than cloned per lecturer: a Session
        // holds at most `1 + MAX_ADDITIONAL_ROOMS` Rooms, and this way the
        // callers' iterators need not be `Clone`.
        let mut held = [None; 1 + MAX_ADDITIONAL_ROOMS];
        for (i, r) in rooms.take(held.len()).enumerate() {
            held[i] = Some(r);
        }
        lecturers.into_iter().any(|l| {
            self.person_room_veto
                .get(l.get())
                .is_some_and(|veto| held.iter().flatten().any(|r| veto.contains(r.get())))
        })
    }

    /// SOFT. Charges a placement for sitting inside this Offering's exam
    /// period — or, with an `invert` instance, for sitting outside it.
    /// ADR-0033.
    ///
    /// Every decision is precomputed into [`Offering::exam_week_slots`]:
    /// which weeks are exam weeks, which of those are scoped to Groups, and
    /// whether this Offering's cohorts are among them. So this stays a bit
    /// test on the hot path, the same shape [`Self::specialized_room_cost`]
    /// has.
    ///
    /// Keyed on the START slot alone, matching what the soft table charged
    /// when this type lived there: a Session's span cannot leave its week.
    ///
    /// THE GUARD IS ON THE CHARGES, NEVER ON THE MASK. An empty
    /// `exam_week_slots` means this Offering has no exam period, and under
    /// `invert` that means charged at EVERY slot — a non-steering constant
    /// that is nonetheless the honest reading, and exactly what an `invert`
    /// instance over a calendar with no exam weeks already charged before
    /// scoping existed. Short-circuiting on an empty mask would make the
    /// inverted direction silently cost nothing.
    #[inline]
    pub fn exam_week_cost(&self, offering: &Offering, start: SlotIdx) -> f64 {
        if offering.exam_week_charge_in == 0.0 && offering.exam_week_charge_out == 0.0 {
            return 0.0;
        }
        if offering.exam_week_slots.contains(start.get()) {
            offering.exam_week_charge_in
        } else {
            offering.exam_week_charge_out
        }
    }

    /// SOFT. Discourages a Session's span from crossing a `grid_time` gap —
    /// starting before a break and resuming after it. Charges each covering
    /// instance's flat `weight` once if the span crosses ANY gap at all,
    /// never scaled by minutes or gap count: the bound `hard_penalty` relies
    /// on is that this term's ceiling per placement is `weight` itself, the
    /// same shape `capacity_waste_cost`'s saturating curve gives.
    ///
    /// Zero for any single-block Session — `gap_minutes_within_span` already
    /// returns 0 for a span with no interior — and zero whenever no instance
    /// is configured, so a constraint set without this type never even
    /// resolves `slots.flags`.
    #[inline]
    pub fn break_spanning_cost(
        &self,
        offering: &Offering,
        start: SlotIdx,
        duration_blocks: u32,
    ) -> f64 {
        if self.constraints.minimize_break_spanning.is_empty() {
            return 0.0;
        }
        let f = self.slots.flags(start);
        let minutes =
            self.grid_time
                .gap_minutes_within_span(f.iso_weekday, f.block, duration_blocks);
        if minutes == 0 {
            return 0.0;
        }
        self.constraints
            .minimize_break_spanning
            .iter()
            .filter(|i| i.covers(&offering.kind))
            .map(|i| i.weight)
            .sum()
    }

    /// The dense location index `Room.location` was interned to — the key
    /// `MinimizeLocationChange` groups distinct Rooms by. Two Rooms share an
    /// index exactly when their `location` strings are equal.
    #[inline]
    pub fn room_location(&self, r: RoomIdx) -> u32 {
        self.room_location[r.get()]
    }

    /// Every OTHER exclusive Room that cannot be in use while `r` is — `r`'s
    /// [`Room::footprints`] resolved against every other Room's.
    ///
    /// Read on the QUERY side only. `Occupancy` marks one bit per Room a
    /// Session is actually assigned, and asks this on `is_free`; the reverse —
    /// marking the siblings — would make overlap TRANSITIVE, and it is not.
    /// With `A | mid | B` behind two separate folding walls, `mid` overlaps
    /// both and `A` overlaps neither `B` nor anything of `B`'s; booking `A`
    /// must leave `B` bookable. Expanding the question preserves that;
    /// expanding the answer loses it.
    ///
    /// EXCLUDES `r` itself, so the common case is an EMPTY slice and the
    /// occupancy hot loop keeps its single `get` on `r`'s own row with a
    /// zero-iteration loop after it. Excludes non-exclusive Rooms for
    /// ADR-0022's reason: a virtual Room has no physical footprint and its
    /// occupancy row is never read, so a tag on one could only ever block
    /// something it does not stand in.
    ///
    /// Symmetric: `b` is in `a`'s siblings exactly when `a` is in `b`'s, so
    /// whichever of the two is placed first, the other is refused.
    #[inline]
    pub fn footprint_siblings(&self, r: RoomIdx) -> &[RoomIdx] {
        &self.room_footprint_siblings[r.get()]
    }

    /// A stable label for a placement, preferring the existing Session id.
    pub fn placement_label(&self, p: PlacementIdx) -> String {
        let var = self.placement(p);
        var.existing_session_id
            .clone()
            .unwrap_or_else(|| format!("{}#{}", self.offering_of(p).id, var.occurrence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slots::{SlotTable, WeekKind, WeekSpec};

    fn grid() -> SlotTable {
        SlotTable::build(
            2,
            &[1],
            &[WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] }],
        )
        .unwrap()
    }

    fn group(id: &str, parent: Option<u32>) -> Group {
        Group {
            id: id.to_string(),
            parent: parent.map(GroupIdx),
            name: id.to_string(),
            size: 0,
            blackouts: vec![],
        }
    }

    #[test]
    fn attendance_pulls_in_descendants_but_not_ancestors() {
        // A(0) -> B(1) -> C(2)
        let groups = vec![group("A", None), group("B", Some(0)), group("C", Some(1))];
        let persons = vec![
            Person {
                id: "pa".into(),
                role_tags: vec![],
                groups: vec![GroupIdx(0)],
                blackouts: vec![],
                preferred: None,
                allowed_rooms: vec![],
            },
            Person {
                id: "pb".into(),
                role_tags: vec![],
                groups: vec![GroupIdx(1)],
                blackouts: vec![],
                preferred: None,
                allowed_rooms: vec![],
            },
            Person {
                id: "pc".into(),
                role_tags: vec![],
                groups: vec![GroupIdx(2)],
                blackouts: vec![],
                preferred: None,
                allowed_rooms: vec![],
            },
        ];

        let specs = vec![
            OfferingSpec {
                id: "top".into(),
                kind: "lecture".into(),
                required_session_count: 1,
                duration_blocks: 1,
                lecturers: vec![],
                eligible_lecturer_combinations: vec![],
                groups: vec![GroupIdx(0)],
                participants: vec![],
                eligible_rooms: vec![],
                required_room_count: 0,
                eligible_room_combinations: vec![],
                min_capacity: 0,
                required_room_features: vec![],
                scheduling_pattern: SchedulingPattern::Unspecified,
                prefer_fuller_days: false,
            },
            OfferingSpec {
                id: "leaf".into(),
                kind: "lecture".into(),
                required_session_count: 1,
                duration_blocks: 1,
                lecturers: vec![],
                eligible_lecturer_combinations: vec![],
                groups: vec![GroupIdx(2)],
                participants: vec![],
                eligible_rooms: vec![],
                required_room_count: 0,
                eligible_room_combinations: vec![],
                min_capacity: 0,
                required_room_features: vec![],
                scheduling_pattern: SchedulingPattern::Unspecified,
                prefer_fuller_days: false,
            },
        ];

        let mut spec =
            ProblemSpec { groups, persons, offerings: specs, ..ProblemSpec::new(grid()) };
        spec.expand_placements();
        let p = Problem::build(spec).unwrap();

        // A session for the cohort involves everyone beneath it.
        assert_eq!(p.offerings[0].attendees, vec![PersonIdx(0), PersonIdx(1), PersonIdx(2)]);
        // A session for the deepest group involves only its own member.
        assert_eq!(p.offerings[1].attendees, vec![PersonIdx(2)]);

        // But conflict propagation still goes BOTH ways.
        assert!(p.offerings[1].conflict_groups.contains(&GroupIdx(0)));
    }

    fn spec_with(offerings: Vec<OfferingSpec>) -> ProblemSpec {
        let mut spec = ProblemSpec { offerings, ..ProblemSpec::new(grid()) };
        spec.expand_placements();
        spec
    }

    fn offering(id: &str, required: u32) -> OfferingSpec {
        OfferingSpec {
            id: id.into(),
            kind: "lecture".into(),
            required_session_count: required,
            duration_blocks: 1,
            lecturers: vec![],
            eligible_lecturer_combinations: vec![],
            groups: vec![],
            participants: vec![],
            eligible_rooms: vec![],
            required_room_count: 0,
            eligible_room_combinations: vec![],
            min_capacity: 0,
            required_room_features: vec![],
            scheduling_pattern: SchedulingPattern::Unspecified,
            prefer_fuller_days: false,
        }
    }

    #[test]
    fn scope_defaults_to_every_offering() {
        let p = Problem::build(spec_with(vec![offering("a", 1), offering("b", 2)])).unwrap();
        assert!(p.offering_ids().all(|o| p.in_scope(o)));
    }

    #[test]
    fn declared_scope_excludes_the_offerings_it_omits() {
        let mut spec = spec_with(vec![offering("a", 1), offering("b", 2)]);
        spec.scope = ScopeSpec::Offerings(vec![OfferingIdx(1)]);
        let p = Problem::build(spec).unwrap();

        assert!(!p.in_scope(OfferingIdx(0)));
        assert!(p.in_scope(OfferingIdx(1)));
    }

    #[test]
    fn residual_is_zero_when_placements_cover_the_required_count() {
        let p = Problem::build(spec_with(vec![offering("a", 3)])).unwrap();
        assert_eq!(p.placement_count(OfferingIdx(0)), 3);
        assert_eq!(p.residual_for(OfferingIdx(0)), 0);
    }

    #[test]
    fn residual_goes_negative_when_locks_over_supply_the_offering() {
        // Two required, four immovable Sessions already linked to it. The
        // expansion saturates to zero placements, and the surplus shows up as a
        // negative residual rather than vanishing.
        let mut spec = ProblemSpec {
            offerings: vec![offering("a", 2)],
            fixed: (0..4)
                .map(|i| FixedSpec {
                    session_id: format!("s{i}"),
                    external: false,
                    offering: Some(OfferingIdx(0)),
                    kind: "lecture".into(),
                    room: None,
                    additional_rooms: [None; MAX_ADDITIONAL_ROOMS],
                    start: SlotIdx(0),
                    duration_blocks: 1,
                    lecturers: vec![],
                    groups: vec![],
                    persons: vec![],
                    reason: Immovable::Locked,
                })
                .collect(),
            ..ProblemSpec::new(grid())
        };
        spec.expand_placements();
        let p = Problem::build(spec).unwrap();

        assert_eq!(p.placement_count(OfferingIdx(0)), 0, "saturated, never wrapped");
        assert_eq!(p.immovable_count(OfferingIdx(0)), 4);
        assert_eq!(p.residual_for(OfferingIdx(0)), -2, "over-supplied by two");
        assert!(
            p.in_scope(OfferingIdx(0)),
            "zero placements must not be mistaken for out of scope — that is \
             precisely what made over-supply unreportable"
        );
    }

    #[test]
    fn kind_scoping_selects_which_checks_apply() {
        let set = ConstraintSet {
            group_double_booking: vec![ConstraintInstance {
                id: "g".into(),
                kinds: vec!["lecture".into()],
            }],
            room_double_booking: vec![ConstraintInstance { id: "r".into(), kinds: vec![] }],
            ..Default::default()
        };

        // A groupless tenant kind is not subject to the group check...
        let staff = set.enforce_for_kind("staff_meeting");
        assert!(!staff.group);
        // ...but an all-kinds instance still applies.
        assert!(staff.room);

        let lecture = set.enforce_for_kind("lecture");
        assert!(lecture.group && lecture.room);
    }

    /// A movable, OUT-OF-SCOPE `PlacementVar`, `original` set, on a spec NOT
    /// passed through `expand_placements` — that helper unconditionally
    /// rebuilds `placements` from `offerings.required_session_count` with
    /// `original: None`, which is exactly the v1 shape this is testing past.
    /// Out of scope is what selects `movement_weight` over
    /// `in_scope_movement_weight` in `movement_cost` — see
    /// [`in_scope_movable_spec`] for the other side.
    fn movable_spec(weight: f64) -> ProblemSpec {
        ProblemSpec {
            offerings: vec![offering("o", 1)],
            placements: vec![PlacementVar {
                offering: OfferingIdx(0),
                occurrence: 0,
                existing_session_id: Some("s1".into()),
                original: Some((SlotIdx(0), Some(RoomIdx(0)))),
            }],
            movement_weight: weight,
            scope: ScopeSpec::Offerings(vec![]),
            ..ProblemSpec::new(grid())
        }
    }

    /// The in-scope counterpart of [`movable_spec`]: same reused placement,
    /// but the Offering IS in scope, so `movement_cost` must select
    /// `in_scope_movement_weight` instead.
    fn in_scope_movable_spec(weight: f64) -> ProblemSpec {
        ProblemSpec {
            offerings: vec![offering("o", 1)],
            placements: vec![PlacementVar {
                offering: OfferingIdx(0),
                occurrence: 0,
                existing_session_id: Some("s1".into()),
                original: Some((SlotIdx(0), Some(RoomIdx(0)))),
            }],
            in_scope_movement_weight: weight,
            ..ProblemSpec::new(grid())
        }
    }

    #[test]
    fn movement_cost_is_zero_back_at_the_original_placement() {
        let p = Problem::build(movable_spec(3.0)).unwrap();
        assert_eq!(p.movement_cost(PlacementIdx(0), SlotIdx(0), RoomIdx(0)), 0.0);
    }

    #[test]
    fn movement_cost_charges_the_weight_for_a_slot_change() {
        let p = Problem::build(movable_spec(3.0)).unwrap();
        assert_eq!(p.movement_cost(PlacementIdx(0), SlotIdx(1), RoomIdx(0)), 3.0);
    }

    #[test]
    fn movement_cost_charges_the_weight_for_a_room_only_change() {
        // The decision this test pins down: "minimize movement" is one knob,
        // not a slot/room split. Same slot, different room, still charged —
        // there is no cheaper way to leave a Session than to keep it exactly
        // where it was.
        let p = Problem::build(movable_spec(3.0)).unwrap();
        assert_eq!(p.movement_cost(PlacementIdx(0), SlotIdx(0), RoomIdx(1)), 3.0);
    }

    #[test]
    fn movement_cost_is_zero_without_an_original_regardless_of_weight() {
        // A brand-new Session — the overwhelming majority of any run's
        // placements — has no `original`. This is what makes it safe for
        // `hard_penalty` and `initial_temperature` to fold both movement
        // weights in unconditionally: a placement neither term applies to
        // costs it nothing.
        let mut spec = movable_spec(3.0);
        spec.placements[0].original = None;
        let p = Problem::build(spec).unwrap();
        assert_eq!(p.movement_cost(PlacementIdx(0), SlotIdx(1), RoomIdx(1)), 0.0);
    }

    #[test]
    fn in_scope_movement_cost_is_zero_back_at_the_original_placement() {
        let p = Problem::build(in_scope_movable_spec(3.0)).unwrap();
        assert_eq!(p.movement_cost(PlacementIdx(0), SlotIdx(0), RoomIdx(0)), 0.0);
    }

    #[test]
    fn in_scope_movement_cost_charges_the_in_scope_weight_for_a_slot_change() {
        let p = Problem::build(in_scope_movable_spec(3.0)).unwrap();
        assert_eq!(p.movement_cost(PlacementIdx(0), SlotIdx(1), RoomIdx(0)), 3.0);
    }

    #[test]
    fn an_out_of_scope_placement_never_reads_the_in_scope_weight() {
        // `movable_spec` is out of scope and sets ONLY `movement_weight`;
        // `in_scope_movement_weight` defaults to 0.0. If `movement_cost` ever
        // mixed the two up, this would charge nothing instead of `weight`.
        let p = Problem::build(movable_spec(3.0)).unwrap();
        assert_eq!(p.movement_cost(PlacementIdx(0), SlotIdx(1), RoomIdx(0)), 3.0);
    }

    #[test]
    fn an_in_scope_placement_never_reads_the_out_of_scope_weight() {
        // `in_scope_movable_spec` is in scope and sets ONLY
        // `in_scope_movement_weight`; `movement_weight` defaults to 0.0. If
        // `movement_cost` ever mixed the two up, this would charge nothing.
        let p = Problem::build(in_scope_movable_spec(3.0)).unwrap();
        assert_eq!(p.movement_cost(PlacementIdx(0), SlotIdx(1), RoomIdx(0)), 3.0);
    }

    // -- per-entity movement overrides (issue #70) --------------------------

    fn person(id: &str, groups: &[u32]) -> Person {
        Person {
            id: id.into(),
            role_tags: vec!["lecturer".into()],
            groups: groups.iter().map(|&g| GroupIdx(g)).collect(),
            blackouts: vec![],
            preferred: None,
            allowed_rooms: vec![],
        }
    }

    /// `in_scope_movable_spec` with a two-level Group tree (`programme` ->
    /// `cohort`), one lecturer, and the Offering attached to the LEAF Group.
    /// Both axes an override can arrive on are then live at once.
    fn overridable_spec(base: f64) -> ProblemSpec {
        let mut spec = in_scope_movable_spec(base);
        spec.groups = vec![group("programme", None), group("cohort", Some(0))];
        spec.persons = vec![person("lecturer", &[]), person("other", &[])];
        spec.offerings[0].lecturers = vec![PersonIdx(0)];
        spec.offerings[0].groups = vec![GroupIdx(1)];
        spec
    }

    /// The cost of moving the one placement, which is all these assert on.
    fn moved(spec: ProblemSpec) -> f64 {
        Problem::build(spec)
            .unwrap()
            .movement_cost(PlacementIdx(0), SlotIdx(1), RoomIdx(0))
    }

    #[test]
    fn no_override_leaves_the_scope_wide_weight_untouched() {
        assert_eq!(moved(overridable_spec(3.0)), 3.0);
    }

    #[test]
    fn a_person_override_replaces_the_scope_wide_weight_for_a_session_they_lecture() {
        let mut spec = overridable_spec(3.0);
        spec.movement_overrides.persons = vec![(PersonIdx(0), 50.0)];
        assert_eq!(moved(spec), 50.0, "replaces, never adds to or scales, the base");
    }

    #[test]
    fn a_person_override_of_zero_makes_a_session_free_to_move() {
        // Half the issue's ask: "movable, no extra cost" has to survive a
        // large scope-wide weight, which is why an override REPLACES rather
        // than maxes against the base.
        let mut spec = overridable_spec(500.0);
        spec.movement_overrides.persons = vec![(PersonIdx(0), 0.0)];
        assert_eq!(moved(spec), 0.0);
    }

    #[test]
    fn an_override_naming_someone_who_does_not_lecture_this_offering_is_inert() {
        let mut spec = overridable_spec(3.0);
        spec.movement_overrides.persons = vec![(PersonIdx(1), 50.0)];
        assert_eq!(moved(spec), 3.0);
    }

    #[test]
    fn a_group_override_covers_a_session_attached_to_that_group() {
        let mut spec = overridable_spec(3.0);
        spec.movement_overrides.groups = vec![(GroupIdx(1), 50.0)];
        assert_eq!(moved(spec), 50.0);
    }

    #[test]
    fn a_group_override_binds_downward_so_an_ancestors_entry_covers_a_descendants_session() {
        // Declared on the PROGRAMME, and the Offering is attached to the
        // COHORT beneath it. `expand_ancestry`, exactly as `GroupVeto` does
        // (ADR-0027) — a protected programme protects its cohorts.
        let mut spec = overridable_spec(3.0);
        spec.movement_overrides.groups = vec![(GroupIdx(0), 50.0)];
        assert_eq!(moved(spec), 50.0);
    }

    #[test]
    fn a_group_override_does_not_bind_upward() {
        // The mirror of the test above, and the reason `expand_subtree` and
        // `expand_conflict` are both wrong here: an entry on the cohort must
        // not protect a Session the whole programme attends.
        let mut spec = overridable_spec(3.0);
        spec.offerings[0].groups = vec![GroupIdx(0)];
        spec.movement_overrides.groups = vec![(GroupIdx(1), 50.0)];
        assert_eq!(moved(spec), 3.0);
    }

    #[test]
    fn the_largest_matching_override_wins() {
        // Order-independent, so it cannot depend on the order the caller sent
        // them — asserted in both orders — and a broader "movable" never
        // silently defeats a narrower protection.
        let mut spec = overridable_spec(3.0);
        spec.movement_overrides.persons = vec![(PersonIdx(0), 0.0)];
        spec.movement_overrides.groups = vec![(GroupIdx(1), 50.0)];
        assert_eq!(moved(spec), 50.0);

        let mut spec = overridable_spec(3.0);
        spec.movement_overrides.persons = vec![(PersonIdx(0), 50.0)];
        spec.movement_overrides.groups = vec![(GroupIdx(1), 0.0)];
        assert_eq!(moved(spec), 50.0);
    }

    #[test]
    fn a_lecturer_pool_candidate_is_enough_to_carry_the_override() {
        // Deliberately conservative: which pool candidate teaches a Session
        // is a search-time choice, so an exact answer would need pricing per
        // candidate (the trap ADR-0026 records for the preference table).
        // Matching ANY candidate over-protects rather than under-protects,
        // which is the safe direction for a soft bias.
        let mut spec = overridable_spec(3.0);
        spec.offerings[0].lecturers = vec![];
        spec.offerings[0].eligible_lecturer_combinations = vec![
            [Some(PersonIdx(1)), None, None, None],
            [Some(PersonIdx(0)), None, None, None],
        ];
        spec.movement_overrides.persons = vec![(PersonIdx(0), 50.0)];
        assert_eq!(moved(spec), 50.0);
    }

    #[test]
    fn an_override_still_charges_nothing_without_an_original() {
        // An override prices a move; it cannot invent one. A brand-new
        // Session has nowhere it "already was", so there is nothing to
        // charge — the same reason a HARD-locked Session is untouched by
        // this, having no `PlacementVar` at all.
        let mut spec = overridable_spec(3.0);
        spec.placements[0].original = None;
        spec.movement_overrides.persons = vec![(PersonIdx(0), 50.0)];
        assert_eq!(moved(spec), 0.0);
    }

    #[test]
    fn hard_penalty_still_dominates_an_override_larger_than_both_base_weights() {
        // The bound `hard_penalty` relies on is "each term costs at most its
        // own ceiling per placement". An override replaces the base, so the
        // ceiling moved — if `hard_penalty` had not folded the largest
        // override in, a protected Session sitting still could outrank an
        // unplaced one.
        let mut spec = overridable_spec(1.0);
        spec.movement_overrides.persons = vec![(PersonIdx(0), 1_000.0)];
        let p = Problem::build(spec).unwrap();
        assert!(
            p.hard_penalty > p.movement_cost(PlacementIdx(0), SlotIdx(1), RoomIdx(0)),
            "hard_penalty {} must dominate one placement's movement cost",
            p.hard_penalty
        );
    }
}
