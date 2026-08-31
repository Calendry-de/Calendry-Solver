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
};
use crate::bitset::BitSet;
use crate::groups::{GroupClosure, GroupCycle};
use crate::ids::{GroupIdx, OfferingIdx, PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use crate::preferences::{Preference, PreferenceInstance, PreferenceModel};
use crate::slots::{GridTime, SlotTable};
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
    pub federation_owned: bool,
    /// Free-text building/campus identifier. `""` means unconfigured —
    /// naturally inert for `MinimizeLocationChange`, since every Room sharing
    /// that empty string counts as the SAME location rather than as distinct
    /// ones.
    pub location: String,
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
}

/// A blackout window.
///
/// An empty list on an axis means "every value on that axis", so `{days:[5]}`
/// is every Friday and `{blocks:[0]}` is every first block. All three empty
/// therefore means always unavailable, which is the literal reading and is
/// preserved rather than silently treated as "never".
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
}

/// One configured Offering relation — an ordered set of Offering references
/// plus a type, per ADR-0028. `members` is ordered because a FEW future
/// relation types (`Precedence`, `Next Day`) read the order; `DifferentTime`
/// ignores it.
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
}

impl ConstraintSet {
    pub fn enforce_for_kind(&self, kind: &str) -> Enforce {
        Enforce {
            room: any_covers(&self.room_double_booking, kind),
            lecturer: any_covers(&self.lecturer_double_booking, kind),
            group: any_covers(&self.group_double_booking, kind),
            person: any_covers(&self.person_double_booking, kind),
            lecturer_veto: any_covers(&self.lecturer_veto, kind),
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
    /// The tenant-declared minimum, already spent as a HARD eligibility
    /// filter in `convert::build_offerings` (`eligible_rooms`/
    /// `eligible_room_combinations` never contain a Room too small for it).
    /// Kept here too, unlike before, because `MinimizeCapacityWaste` needs
    /// the RAW number to grade how much LARGER the assigned Room is — a
    /// question the eligibility filter's boolean pass/fail already answered
    /// and discarded. `0` means no requirement was ever stated.
    pub min_capacity: u32,
}

/// Immovable occupancy as supplied, before closures are derived.
#[derive(Clone, Debug)]
pub struct FixedSpec {
    pub session_id: String,
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
    /// Dense row indices into the problem's `DifferentTime` relations this
    /// Offering is a member of — empty for every Offering not named in one,
    /// which is the overwhelming majority. Precomputed the same way
    /// `veto_slots` is: derived once in `Problem::build`, read every time a
    /// Session of this Offering is marked, unmarked or probed for a slot.
    pub different_time_relations: Vec<u32>,
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
    /// The grid's wall-clock gap structure — see [`GridTime`]. Defaults to
    /// no gaps anywhere, which is inert unless `MinimizeBreakSpanning` or
    /// `Daybreak` is configured.
    pub grid_time: GridTime,
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
            grid_time: GridTime::default(),
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
    /// The grid's wall-clock gap structure — see [`GridTime`]. Only read by
    /// `MinimizeBreakSpanning` and `Daybreak`.
    pub grid_time: GridTime,
    pub rooms: Vec<Room>,
    pub groups: Vec<Group>,
    pub persons: Vec<Person>,
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
    /// Every configured `DifferentTime` relation's own id, dense, parallel to
    /// the row index every Offering's and `FixedOccupancy`'s
    /// `different_time_relations` entry names. Its length is the row count
    /// the solution module's relation occupancy matrix is sized against.
    pub different_time_relation_ids: Vec<String>,

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
            grid_time,
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
        // mark/unmark" shape `veto_mask` above uses. Other `RelationKind`
        // variants would get their own membership table here; there is only
        // one today.
        let mut different_time_relation_ids: Vec<String> = Vec::new();
        let mut different_time_membership: Vec<Vec<u32>> = vec![Vec::new(); offerings.len()];
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
            }
        }

        let derived_offerings: Vec<Offering> = offerings
            .into_iter()
            .enumerate()
            .map(|(i, o)| Offering {
                different_time_relations: different_time_membership[i].clone(),
                soft_profile: soft.profile_for_kind(&o.kind),
                veto_slots: veto_mask(&o.lecturers),
                group_veto_slots: group_veto_mask(&o.groups),
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
                session_id: f.session_id,
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
        let break_spanning_weight: f64 = constraints
            .minimize_break_spanning
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
         * ceilings is a safe bound, if not the tightest one.
         *
         * THE CAPACITY-WASTE TERM IS BOUNDED THE SAME WAY `MinimizeRoomRank`
         * IS: `capacity_waste_cost`'s saturating curve caps each covering
         * instance's contribution at its own `weight`, so summing every
         * instance's weight is the per-placement ceiling, same shape as
         * `soft.total_weight`.
         *
         * THE BREAK-SPANNING TERM IS BOUNDED THE SAME WAY: `break_spanning_
         * cost` charges each covering instance's flat `weight` at most once
         * per placement (span crosses a gap, or it does not — no scaling by
         * how many minutes or how many gaps), so summing every instance's
         * weight is the per-placement ceiling, same shape as
         * `capacity_waste_weight`.
         */
        let hard_penalty = soft.total_weight * placements.len() as f64
            + preferences.max_cost_per_placement() * placements.len() as f64
            + day_mix_weight * aggregate_template.day_mix_cell_count() as f64
            + movement_weight * placements.len() as f64
            + in_scope_movement_weight * placements.len() as f64
            + capacity_waste_weight * placements.len() as f64
            + break_spanning_weight * placements.len() as f64
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

        let problem = Self {
            slots,
            grid_time,
            rooms,
            groups,
            persons,
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
            room_churn_weight,
            room_consistency_weight,
            lecturer_consistency_weight,
            room_location,
            different_time_relation_ids,
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
    /// Which of the two weights applies is `p`'s Offering's SCOPE, not
    /// whether `original` happens to be set: `movement_weight` for an
    /// out-of-scope Session made movable by `LOCK_POLICY_MINIMIZE_MOVEMENT`,
    /// `in_scope_movement_weight` for an in-scope Session reused by a
    /// targeted repair. The two never charge the same placement, since a
    /// placement's Offering is in scope or is not.
    #[inline]
    pub fn movement_cost(&self, p: PlacementIdx, start: SlotIdx, room: RoomIdx) -> f64 {
        let var = &self.placements[p.get()];
        match var.original {
            Some(original) if original != (start, Some(room)) => {
                if self.in_scope(var.offering) {
                    self.in_scope_movement_weight
                } else {
                    self.movement_weight
                }
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
            },
            Person {
                id: "pb".into(),
                role_tags: vec![],
                groups: vec![GroupIdx(1)],
                blackouts: vec![],
                preferred: None,
            },
            Person {
                id: "pc".into(),
                role_tags: vec![],
                groups: vec![GroupIdx(2)],
                blackouts: vec![],
                preferred: None,
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
                scheduling_pattern: SchedulingPattern::Unspecified,
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
                scheduling_pattern: SchedulingPattern::Unspecified,
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
            scheduling_pattern: SchedulingPattern::Unspecified,
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
}
