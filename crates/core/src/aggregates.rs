//! Day- and window-granularity counters for the two Group-scoped hard types.
//!
//! # Why these are not occupancy bitsets
//!
//! Slices 1–3 needed exactly two shapes: **pairwise** interactions keyed by
//! `(entity, slot)` (the four structural types), and **unary** per-Session costs
//! keyed by `(slot, room)` (the six soft types). Neither shape fits here.
//!
//! * `OnlineOnsiteSameDay` interacts at **day** granularity, not slot. Two
//!   Sessions of one Group clash only if they share a *day* and disagree about
//!   being online — a pair sharing a slot is neither necessary nor sufficient.
//!
//!   It WAS a feasibility filter, because it is monotone-safe: placing the
//!   first Session on a day can never violate it. It is now SOFT, so the filter
//!   is gone and the same counters feed the objective instead. The counters did
//!   not change; what changed is that `day_mix_allows` answers "would this cost
//!   something" rather than "is this permitted at all".
//!
//! * `MaxOnlineShare` is a **cardinality ratio over a set**, and cannot be a
//!   filter at all. "31% online" is invisible in any pair of Sessions, and a
//!   filter would dead-end construction: the first online Session placed makes
//!   the ratio 100%, over any threshold below 1.0, because the denominator has
//!   not grown yet. For `PER_WEEK` the denominator also *moves* when a Session
//!   relocates between weeks.
//!
//!   So it lives on the **objective** instead, on the hard side. A run can
//!   therefore succeed while still reporting a `MaxOnlineShare` violation —
//!   exactly how `ExactFrequency` already behaves for unplaced Sessions, rather
//!   than a new exception.

use crate::ids::{GroupIdx, OfferingIdx, PersonIdx, RoomIdx, SlotIdx};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShareWindow {
    /// One bucket for the whole term.
    PerTerm,
    /// One bucket per calendar week index from the academic calendar.
    PerWeek,
}

/// One configured `OnlineOnsiteSameDay`.
///
/// SOFT since the reclassification. It carries a weight for the same reason
/// every other soft instance does — the objective needs to know what a mixed
/// day costs — and it stays in its own list rather than joining
/// [`crate::soft::SoftModel`] because that model is a precomputed
/// `(slot, room)` table and a mixed day is a property of what ELSE is already
/// placed for the Group that day. It cannot be read off a table keyed by the
/// candidate alone.
#[derive(Clone, Debug)]
pub struct DayMixInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
}

impl DayMixInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MaxOnlineShare`.
#[derive(Clone, Debug)]
pub struct ShareInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    /// 0.0 ..= 1.0
    pub max_ratio: f64,
    pub window: ShareWindow,
}

impl ShareInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }

    /// The permitted number of online Sessions out of `total`.
    ///
    /// Floor, so 3 Sessions at a 0.3 ratio permit **zero** online — the
    /// constraint is a ceiling on the share, and rounding up would silently
    /// allow 33%.
    #[inline]
    pub fn allowance(&self, total: u32) -> u32 {
        (self.max_ratio * total as f64).floor() as u32
    }
}

/// One configured `Compactness`.
///
/// Like `OnlineOnsiteSameDay`, a single shared weight rather than per-instance
/// buckets: `group`/`person` independently gate which axis THIS instance's
/// weight applies to (the wire's `repeated CompactnessScope`, empty = both), so
/// a tenant wanting different weights per axis configures two instances rather
/// than this type growing a second weight field.
#[derive(Clone, Debug)]
pub struct CompactnessInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
    pub group: bool,
    pub person: bool,
}

impl CompactnessInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MaxConsecutiveBlocks` rule — the mirror image of
/// `CompactnessInstance`: same `group`/`person` axis split, plus the run-
/// length cap `Compactness` has no equivalent of.
#[derive(Clone, Debug)]
pub struct MaxConsecutiveInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
    pub group: bool,
    pub person: bool,
    pub max_consecutive: u32,
}

impl MaxConsecutiveInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MaxDailySpan` rule — same `group`/`person` axis split as
/// `CompactnessInstance`/`MaxConsecutiveInstance`, plus the elapsed-time cap
/// neither of those has an equivalent of.
#[derive(Clone, Debug)]
pub struct MaxDailySpanInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
    pub group: bool,
    pub person: bool,
    pub max_span_blocks: u32,
}

impl MaxDailySpanInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MaxDailySessionCount` rule — same `group`/`person` axis
/// split as `MaxDailySpan`, but capping a raw Session COUNT per day rather
/// than elapsed span. SOFT and priced once the cap is exceeded rather than
/// refused, the same reasoning `MaxWeeklyTeachingLoad` and ADR-0025 give: a
/// hard cap on a count only fully known as placements accumulate risks the
/// same dead-end-construction problem.
#[derive(Clone, Debug)]
pub struct MaxDailySessionCountInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
    pub group: bool,
    pub person: bool,
    pub max_per_day: u32,
}

impl MaxDailySessionCountInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MaxWeeklyTeachingLoad` rule. Lecturer-only — unlike
/// `CompactnessInstance` and its siblings, there is no Group/Person axis
/// split, since the whole point is capping the person actually TEACHING.
#[derive(Clone, Debug)]
pub struct MaxWeeklyTeachingLoadInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
    /// Count SESSIONS if false, BLOCKS if true.
    pub count_blocks: bool,
    pub max_per_week: u32,
}

impl MaxWeeklyTeachingLoadInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `ExamSpacingSameDay` rule. Which Sessions count as
/// "exam-kind" is `kinds` (`ConstraintConfig.applies_to_kinds`) — not a
/// separate field here, the same mechanism every kind-scoped type uses.
#[derive(Clone, Debug)]
pub struct ExamSpacingSameDayInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
}

impl ExamSpacingSameDayInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `ExamSpacingWindow` rule — the generalized sibling of
/// `ExamSpacingSameDayInstance` for a tenant that wants more than one clear
/// day between exams.
#[derive(Clone, Debug)]
pub struct ExamSpacingWindowInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
    pub min_days_between: u32,
}

impl ExamSpacingWindowInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MinimizeWeekdayImbalance` rule. Group-only, like
/// `MaxWeeklyTeachingLoad` and `ExamSpacingSameDay`/`Window` — no parameters:
/// variance is read straight off `TimeGrid.active_days`.
#[derive(Clone, Debug)]
pub struct MinimizeWeekdayImbalanceInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
}

impl MinimizeWeekdayImbalanceInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MaxDays` rule. HARD, priced at `hard_penalty` rather than
/// used as a construction filter — the same ADR-0025 stance `MaxOnlineShare`
/// takes. Same `group`/`person` axis split as `MaxConsecutiveInstance` and
/// siblings; no `weight`, because hard-vs-soft is a property of the type and
/// a HARD type's weight is meaningless. Shares its underlying day-occupancy
/// substrate with `MaxConsecutiveDaysInstance` — see `Aggregates::
/// day_cap_group`/`day_cap_person` — the same "worth building on the same
/// accumulator" reasoning the tracking card gives for reusing
/// `MinimizeWeekdayImbalance`'s shape.
#[derive(Clone, Debug)]
pub struct MaxDaysInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub group: bool,
    pub person: bool,
    pub max_days: u32,
}

impl MaxDaysInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MaxConsecutiveDays` rule — same shape as `MaxDaysInstance`,
/// reducing the SAME day-occupancy cell by longest consecutive run instead of
/// distinct count.
#[derive(Clone, Debug)]
pub struct MaxConsecutiveDaysInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub group: bool,
    pub person: bool,
    pub max_consecutive_days: u32,
}

impl MaxConsecutiveDaysInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `Daybreak` rule. SOFT, priced like `RoomTurnaroundBuffer`
/// (a minimum-gap requirement rather than a cap) — HARD would risk making
/// an instance needlessly harder to solve for a welfare preference. Same
/// `group`/`person` axis split as `MaxDailySpanInstance` and siblings.
/// Composes as "whichever binds HARDEST", which for a MINIMUM requirement
/// is the LARGEST configured `min_rest_minutes` — the opposite direction
/// `run_group_threshold`'s "tightest cap = smallest" convention takes,
/// because this is a floor, not a ceiling.
#[derive(Clone, Debug)]
pub struct DaybreakInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
    pub group: bool,
    pub person: bool,
    pub min_rest_minutes: u32,
}

impl DaybreakInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MinimizeLocationChange` rule — same `group`/`person` axis
/// split as `CompactnessInstance` and siblings, plus the distinct-location
/// cap none of those have an equivalent of. A day counts as a violation once
/// it touches MORE than `max_locations_per_day` distinct `Room.location`
/// values.
#[derive(Clone, Debug)]
pub struct MinimizeLocationChangeInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
    pub group: bool,
    pub person: bool,
    pub max_locations_per_day: u32,
}

impl MinimizeLocationChangeInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `TravelTimeBetweenRooms` rule. SOFT, priced like
/// `RoomTurnaroundBuffer` and `Daybreak` (a minimum-gap requirement, so the
/// "tightest" convention is `max`, not `min`). Same `group`/`person` axis
/// split as `MinimizeLocationChangeInstance` — and reads the SAME
/// `Room.location` field that type already does: the wire also carries a
/// dedicated `Room.site`, staged alongside this type, but `location` is
/// already documented as a "building/campus identifier" and introducing a
/// second field for the identical concept would be pure duplication. The
/// unused `site` field is left in the schema rather than removed, since
/// removing it now would mean rewriting an already-made commit.
#[derive(Clone, Debug)]
pub struct TravelTimeInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
    pub group: bool,
    pub person: bool,
    pub min_minutes_between_sites: u32,
}

impl TravelTimeInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `RoomTurnaroundBuffer` rule. Room-keyed rather than
/// Group/Person-keyed — the first aggregate type to be — so it has no
/// `group`/`person` axis split; it cares which ROOM a Session lands in, not
/// who attends it.
#[derive(Clone, Debug)]
pub struct RoomTurnaroundBufferInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
    /// Minimum blocks that must separate two bookings of the same Room.
    pub buffer_blocks: u32,
}

impl RoomTurnaroundBufferInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MinimizeRoomChurn` rule. Group-only, like
/// `MinimizeWeekdayImbalance` and `ExamSpacingSameDay`/`Window` — the "home
/// room" concept is about a Group's own week, with no Person-axis
/// counterpart. Distinct from `MinimizeLocationChange`: this counts distinct
/// ROOMS across a whole WEEK, not distinct LOCATIONS within one day — a
/// Group could churn five Rooms in the same building without ever crossing
/// one, and vice versa.
#[derive(Clone, Debug)]
pub struct MinimizeRoomChurnInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
    /// A week touching more than this many distinct Rooms is penalized, per
    /// Room past the cap.
    pub max_rooms_per_week: u32,
}

impl MinimizeRoomChurnInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `RoomConsistency` rule. No parameters beyond the usual
/// id/kinds/weight: an Offering's "usual" Room is the MODAL one among its
/// own currently-placed Sessions, priced per Session that differs from it.
/// Keyed by OFFERING rather than Group or Person — an aggregate over the
/// WHOLE TERM, unbounded by day or week, the same shape `LecturerConsistency`
/// uses for the lecturer axis. Room assignment never needed a prerequisite
/// the way lecturer choice did: it is not gated behind pool selection.
#[derive(Clone, Debug)]
pub struct RoomConsistencyInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
}

impl RoomConsistencyInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// Three rules sharing one cell, `(offering, day)`, each reducing its row of
/// occupied blocks a different way — issues #34/#35/#29 named the same cell
/// and asked for it to be built once. Storage stays SEPARATE per type rather
/// than literally shared, matching the existing convention for the Group/
/// Person cell (`group_slot`/`run_group_slot`/`span_group_slot` are three
/// arrays of identical shape, not one): each type must cost nothing when its
/// own rule list is empty, and a shared array would defeat that for two
/// types whenever only the third is configured. What IS shared is the
/// algorithm — `run_excess_u32` (already generic) prices the consecutive-run
/// case unchanged, and `run_count_u32` is the one genuinely new reduction.
///
/// SOFT, priced once the cap is exceeded, the same ADR-0025 reasoning as
/// `MaxDailySessionCount`/`MaxWeeklyTeachingLoad`.
/// One configured `MaxConsecutiveOfferingBlocks` rule — the Offering-keyed
/// sibling of `MaxConsecutiveInstance` (Group/Person axis): caps how many
/// blocks of ONE Offering may run back to back in a day, distinguishing an
/// intentional multi-block Session (`Offering.duration_blocks`, one
/// placement) from several separate Sessions of the same Offering landing
/// consecutively by accident.
#[derive(Clone, Debug)]
pub struct MaxConsecutiveOfferingBlocksInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
    pub max_consecutive: u32,
}

impl MaxConsecutiveOfferingBlocksInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MaxOfferingSessionsPerDay` rule — caps a raw Session
/// COUNT of one Offering on one day, so "Maths, 4x a week" means four
/// different days unless a tenant says otherwise. The Offering-keyed
/// sibling of `MaxDailySessionCountInstance` (Group/Person axis).
#[derive(Clone, Debug)]
pub struct MaxOfferingSessionsPerDayInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
    pub max_per_day: u32,
}

impl MaxOfferingSessionsPerDayInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MinimizeOfferingDaySplit` rule. Prices the number of
/// non-contiguous runs of one Offering's Sessions within a day, minus one —
/// zero for a single contiguous run (including a lone Session), growing with
/// each additional separated run. NOT the same question `Compactness` asks:
/// a day packed solid with `English -> Maths -> History -> English` has ZERO
/// gaps (Compactness is silent) and still splits English across two runs
/// (this fires).
#[derive(Clone, Debug)]
pub struct MinimizeOfferingDaySplitInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
}

impl MinimizeOfferingDaySplitInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `LecturerConsistency` rule. No parameters beyond the usual
/// id/kinds/weight — mirrors `RoomConsistencyInstance`, but the quantity
/// priced is distinct LECTURER identities used across an Offering's Sessions
/// rather than modal-Room misses:
/// `max(0, distinct_lecturers - required_lecturer_count)`. Only Offerings
/// with a genuine lecturer pool
/// (`Offering::has_lecturer_pool`) can ever contribute — a fixed assignment's
/// lecturer set never changes, so its distinct count is always exactly
/// `required_lecturer_count` and this rule can never fire for it.
#[derive(Clone, Debug)]
pub struct LecturerConsistencyInstance {
    pub id: String,
    pub kinds: Vec<String>,
    pub weight: f64,
}

impl LecturerConsistencyInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `DistributedPatternAdherence` or `BlockPatternAdherence` —
/// identical shape, kept as one type since both are just "id, kind scope,
/// weight" with the actual per-pattern logic living in `Aggregates` instead.
#[derive(Clone, Debug)]
pub struct PatternAdherenceInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
}

impl PatternAdherenceInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// Per-instance counters for one `MaxOnlineShare` rule.
#[derive(Clone, Debug)]
struct ShareCounters {
    total: Vec<u32>,
    online: Vec<u32>,
    windows: usize,
    violated: u32,
}

impl ShareCounters {
    fn new(groups: usize, windows: usize) -> Self {
        Self {
            total: vec![0; groups * windows],
            online: vec![0; groups * windows],
            windows,
            violated: 0,
        }
    }

    #[inline]
    fn cell(&self, group: GroupIdx, window: usize) -> usize {
        group.get() * self.windows + window
    }

    #[inline]
    fn is_violated(&self, rule: &ShareInstance, cell: usize) -> bool {
        self.online[cell] > rule.allowance(self.total[cell])
    }
}

#[derive(Clone, Debug, Default)]
pub struct Aggregates {
    /// `[group * n_days + day]` — Sessions of that Group on that day, split by
    /// delivery mode. `OnlineOnsiteSameDay` is violated when both are non-zero.
    online_day: Vec<u32>,
    onsite_day: Vec<u32>,
    n_days: usize,

    rules: Vec<ShareInstance>,
    counters: Vec<ShareCounters>,

    /// `[group * n_slots + slot]` — how many currently-placed Sessions occupy
    /// that Group's block. A COUNT, not a bitset: unlike the structural
    /// `group_double_booking` bitset (which `is_free` enforces so overlap is
    /// impossible), Compactness is soft and never filters placement, so two
    /// Sessions CAN legitimately share a block when the hard type is disabled
    /// — removing one must not clear a block the other still occupies. Empty
    /// when no configured `Compactness` covers the Group axis, so the
    /// allocation and every read/write below is skipped entirely.
    group_slot: Vec<u32>,
    /// The Person counterpart of `group_slot`, one byte per cell rather than
    /// four. Bounded at `u8::MAX` overlapping Sessions per block, which is not
    /// a real ceiling — no real tenant clusters 256 concurrent bookings on one
    /// person — only a saturation guard, because at large-university scale
    /// `persons × slots` is the one array in this module large enough (tens of
    /// MB) for the byte-vs-word choice to matter.
    person_slot: Vec<u8>,
    /// Running sum of `group_day_gap`/`person_day_gap` over every currently
    /// occupied `(entity, day)` cell — maintained as a delta on every mark/
    /// unmark, exactly like `ShareCounters::violated`, rather than rescanned.
    /// A full rescan is what `day_mix_violations` does, and that is safe ONLY
    /// because it is `O(groups × days)`; `persons × days` is roughly 10x
    /// larger at large-university scale and `Trial::objective` reads this on
    /// every accept/reject decision, not just at reporting time — a rescan
    /// there would turn LNS itself slow rather than merely reporting slowly.
    group_gap_total: u32,
    person_gap_total: u32,
    n_slots: usize,
    blocks_per_day: usize,

    compactness_rules: Vec<CompactnessInstance>,

    /// `[offering * weekly_cells + cell]` — how many of that Offering's
    /// currently-placed Sessions occupy that weekly `(weekday, block)` slot.
    /// Same "no week axis" collapse `PreferenceModel`'s table uses, and for
    /// the same reason: a slot Offering O uses in week 3 is the same slot for
    /// this purpose as one it uses in week 7. Empty when
    /// `DistributedPatternAdherence` is not configured for any kind.
    distributed_cell: Vec<u32>,
    /// `[offering]` — how many DISTINCT weekly cells are currently nonzero for
    /// that Offering. `distributed_total` is the sum of `nonzero - 1` (floored
    /// at 0) over every Offering, maintained as a delta exactly like the gap
    /// totals above, not rescanned.
    distributed_nonzero: Vec<u32>,
    distributed_total: u32,
    weekly_cells: usize,

    /// `[offering * n_weeks + week]` — the `BlockPatternAdherence` counterpart
    /// of `group_slot`, at week granularity instead of block granularity and
    /// scoped by Offering instead of Group/Person. Empty when
    /// `BlockPatternAdherence` is not configured for any kind.
    block_week: Vec<u32>,
    block_gap_total: u32,
    n_weeks: usize,

    distributed_rules: Vec<PatternAdherenceInstance>,
    block_rules: Vec<PatternAdherenceInstance>,

    /// `[group * n_slots + slot]` — the `MaxConsecutiveBlocks` counterpart of
    /// `group_slot`: same shape, same reason it is a count rather than a
    /// bitset, but read by `run_excess_u32` (longest RUN of consecutive
    /// occupied blocks) instead of `gap_u32` (idle blocks between the ends).
    /// A separate array from `group_slot` even though both track "is this
    /// block occupied" — `Compactness` and `MaxConsecutiveBlocks` are
    /// independently switchable, so one may be configured without the other.
    run_group_slot: Vec<u32>,
    /// The Person counterpart, `u8` for the same footprint reason
    /// `person_slot` is.
    run_person_slot: Vec<u8>,
    /// The TIGHTEST `max_consecutive` among every enabled instance covering
    /// each axis — multiple instances compose as "whichever binds hardest",
    /// the same convention `Problem::max_concurrent_online` uses.
    /// `u32::MAX` (never exceeded) when nothing configures that axis.
    run_group_threshold: u32,
    run_person_threshold: u32,
    /// Running sum of excess-over-threshold blocks over every currently
    /// occupied `(entity, day)` cell, maintained as a delta exactly like
    /// `group_gap_total`/`person_gap_total`.
    run_group_excess_total: u32,
    run_person_excess_total: u32,

    max_consecutive_rules: Vec<MaxConsecutiveInstance>,

    /// `[group * n_slots + slot]` — the `MaxDailySpan` counterpart of
    /// `group_slot`/`run_group_slot`: same shape again, read by
    /// `span_excess_u32` (elapsed blocks from first to last occupied,
    /// past the cap) instead of `gap_u32`/`run_excess_u32`. Its own array
    /// for the same independent-switch reason those two have theirs.
    span_group_slot: Vec<u32>,
    span_person_slot: Vec<u8>,
    /// The TIGHTEST `max_span_blocks` among every enabled instance covering
    /// each axis, same convention `run_group_threshold`/`run_person_threshold`
    /// use. `u32::MAX` when nothing configures that axis.
    span_group_threshold: u32,
    span_person_threshold: u32,
    span_group_excess_total: u32,
    span_person_excess_total: u32,

    max_daily_span_rules: Vec<MaxDailySpanInstance>,

    /// `[entity * n_days + day]` — how many Sessions this Group/Person
    /// currently has on this day, across the whole term. Same day indexing
    /// as `imbalance_day` (an absolute day index, not per-week-of-day), but
    /// this is a running count read against a fixed cap rather than a
    /// per-week window read for variance.
    daily_count_group: Vec<u32>,
    daily_count_person: Vec<u32>,
    /// The TIGHTEST `max_per_day` among every enabled instance covering each
    /// axis, same "whichever binds hardest" convention as the other
    /// thresholds here. `u32::MAX` when nothing configures that axis.
    daily_count_group_threshold: u32,
    daily_count_person_threshold: u32,
    daily_count_group_excess_total: u32,
    daily_count_person_excess_total: u32,

    daily_count_rules: Vec<MaxDailySessionCountInstance>,

    /// `[offering * n_days + day]` — how many Sessions of this Offering
    /// currently sit on this day, across the whole term. The Offering-keyed
    /// counterpart of `daily_count_group`/`daily_count_person`; a separate
    /// array rather than a shared one for the same independent-switch reason
    /// those two have theirs. See [`MaxOfferingSessionsPerDayInstance`].
    offering_daily_count: Vec<u32>,
    offering_daily_count_threshold: u32,
    offering_daily_count_excess_total: u32,
    offering_daily_count_rules: Vec<MaxOfferingSessionsPerDayInstance>,

    /// `[offering * n_slots + slot]` — occurrence count of this Offering's
    /// currently-placed Sessions at each slot, read a day at a time by
    /// `run_excess_u32` (already generic — the Group/Person axis's own
    /// function, reused unchanged) for the longest-consecutive-run excess.
    /// See [`MaxConsecutiveOfferingBlocksInstance`].
    offering_run_slot: Vec<u32>,
    offering_run_threshold: u32,
    offering_run_excess_total: u32,
    offering_run_rules: Vec<MaxConsecutiveOfferingBlocksInstance>,

    /// `[offering * n_slots + slot]` — same shape as `offering_run_slot`
    /// again, kept as its OWN array for the same independent-switch reason,
    /// read by `run_count_u32` (the number of separate maximal runs, not
    /// their length) for `MinimizeOfferingDaySplit`'s own reduction.
    offering_split_slot: Vec<u32>,
    offering_split_excess_total: u32,
    offering_split_rules: Vec<MinimizeOfferingDaySplitInstance>,

    /// `[person * n_weeks + week]` — how many Sessions (or blocks, per
    /// `teaching_load_count_blocks`) that Person currently leads that week.
    /// Empty when `MaxWeeklyTeachingLoad` is not configured.
    teaching_load_week: Vec<u32>,
    /// The tightest `max_per_week` among every enabled instance, same
    /// "whichever binds hardest" convention as the other thresholds here.
    teaching_load_threshold: u32,
    /// Whether to count blocks rather than Sessions — taken from whichever
    /// instance supplies `teaching_load_threshold`, since one shared counter
    /// cannot serve two different counting rules at once; a tenant mixing
    /// session-counting and block-counting instances gets the tightest
    /// instance's own choice. Practically always one instance.
    teaching_load_count_blocks: bool,
    teaching_load_excess_total: u32,

    teaching_load_rules: Vec<MaxWeeklyTeachingLoadInstance>,

    /// `[group * n_days + day]` — the `ExamSpacingSameDay` counterpart of
    /// `online_day`/`onsite_day`: how many exam-kind Sessions (per
    /// `applies_to_kinds`) that Group currently has on that day. Violated
    /// (2+) is read off fresh each time, like `day_mix_violations` — safe at
    /// the same O(groups x days) scale that type already accepts.
    exam_same_day: Vec<u32>,
    exam_same_day_rules: Vec<ExamSpacingSameDayInstance>,

    /// `[group * n_days + day]` — the `ExamSpacingWindow` counterpart, one
    /// more array because the two types are independently switchable and
    /// independently kind-scoped.
    exam_window: Vec<u32>,
    /// The TIGHTEST `min_days_between` among every enabled instance, same
    /// "whichever binds hardest" convention as the other thresholds here.
    /// `u32::MAX` when not configured.
    exam_window_threshold: u32,
    exam_window_rules: Vec<ExamSpacingWindowInstance>,

    /// `[group * n_days + day_index]` — the `MinimizeWeekdayImbalance`
    /// counterpart of `exam_same_day`: how many Sessions that Group
    /// currently has on that day. Read fresh, like `exam_same_day` and
    /// `day_mix` — see `imbalance_cost`.
    imbalance_day: Vec<u32>,
    /// `TimeGrid.active_days.len()` — `day_index` is already `week *
    /// active_days_count + weekday_position` (see `SlotTable::span`'s own
    /// doc), so one WEEK's cells are exactly `active_days_count` consecutive
    /// `day_index` values; no separate week/weekday decomposition needed.
    active_days_count: usize,
    imbalance_rules: Vec<MinimizeWeekdayImbalanceInstance>,

    /// `[entity * n_days + day_index]` — Sessions of that Group/Person on
    /// that day. The shared day-occupancy substrate `MaxDays` and
    /// `MaxConsecutiveDays` both reduce over one week's slice (width
    /// `active_days_count`, same layout `imbalance_day` uses), one counting
    /// DISTINCT days used and the other the longest CONSECUTIVE run. A
    /// separate array from `imbalance_day` even though both are "Sessions
    /// per (entity, day)": `MinimizeWeekdayImbalance` is Group-only and these
    /// two need the Person axis too, and the three types are independently
    /// switchable. Read fresh, like `imbalance_day` — see
    /// `max_days_violations`/`max_consecutive_days_violations`.
    day_cap_group: Vec<u32>,
    day_cap_person: Vec<u32>,
    /// Tightest `max_days` among every enabled instance covering each axis —
    /// the "whichever binds hardest" convention `run_group_threshold` uses.
    /// `u32::MAX` (never exceeded) when nothing configures that axis.
    max_days_group_threshold: u32,
    max_days_person_threshold: u32,
    /// The `MaxConsecutiveDays` counterpart, same convention.
    max_consecutive_days_group_threshold: u32,
    max_consecutive_days_person_threshold: u32,
    max_days_rules: Vec<MaxDaysInstance>,
    max_consecutive_days_rules: Vec<MaxConsecutiveDaysInstance>,

    /// `[entity * n_slots + slot]` — the `Daybreak` counterpart of
    /// `span_group_slot`/`span_person_slot`: same per-block occupancy
    /// shape, but read across a DAY BOUNDARY (this day's last occupied
    /// block against the next teaching day's first) rather than within one
    /// day, so no within-day excess total is maintained here — the cost
    /// belongs to a PAIR of days, not to any one placement, and is read
    /// fresh like `imbalance_day`.
    daybreak_group_slot: Vec<u32>,
    daybreak_person_slot: Vec<u8>,
    /// Tightest (LARGEST) `min_rest_minutes` among every enabled instance
    /// covering each axis. Unlike a CAP's "tightest = smallest" convention
    /// (`run_group_threshold`), this is a FLOOR: the hardest-to-satisfy
    /// requirement is the longest rest demanded. `0` (never binding) when
    /// nothing configures that axis.
    daybreak_group_threshold_minutes: u32,
    daybreak_person_threshold_minutes: u32,
    daybreak_rules: Vec<DaybreakInstance>,

    /// `[entity * n_slots + slot]` — the `TravelTimeBetweenRooms` counterpart
    /// of `daybreak_group_slot`/`daybreak_person_slot`, but storing the
    /// occupying Room's dense index (`u32::MAX` = unoccupied) rather than a
    /// count, since this type needs to compare WHICH Room, not merely
    /// whether one is occupied. Read across ADJACENT blocks within one day
    /// (never across a day boundary, unlike `Daybreak`), so no within-day
    /// excess total is maintained — read fresh, like every cross-placement
    /// aggregate above.
    travel_group_slot: Vec<u32>,
    travel_person_slot: Vec<u32>,
    /// Tightest (LARGEST) `min_minutes_between_sites` among every enabled
    /// instance covering each axis — a FLOOR, same convention
    /// `daybreak_group_threshold_minutes` uses. `0` (never binding) when
    /// nothing configures that axis.
    travel_group_threshold_minutes: u32,
    travel_person_threshold_minutes: u32,
    travel_rules: Vec<TravelTimeInstance>,

    /// `[group * n_days * n_locations + day * n_locations + loc]` —
    /// occurrence count of Sessions this Group has in each distinct
    /// `Room.location` on each day. The inner axis is LOCATION rather than
    /// slot or day alone, unlike every array above.
    location_group_loc: Vec<u32>,
    /// The Person counterpart.
    location_person_loc: Vec<u32>,
    /// `[group * n_days + day]` — how many DISTINCT locations this Group's
    /// Sessions currently touch that day. Maintained incrementally as
    /// `location_group_loc` cells cross 0<->positive: an exact integer
    /// transition, unlike the `imbalance`/`exam_same_day` family's fresh
    /// rescans, which exist specifically to sidestep floating-point drift
    /// that does not apply to a plain distinct count.
    location_group_distinct: Vec<u32>,
    location_person_distinct: Vec<u32>,
    /// The TIGHTEST `max_locations_per_day` among every enabled instance
    /// covering each axis, same "whichever binds hardest" convention as
    /// `span_group_threshold`/`span_person_threshold`. `u32::MAX` when
    /// nothing configures that axis.
    location_group_threshold: u32,
    location_person_threshold: u32,
    /// Running sum of excess-over-threshold distinct locations over every
    /// currently occupied `(entity, day)` cell, maintained as a delta exactly
    /// like `span_group_excess_total`/`span_person_excess_total`.
    location_group_excess_total: u32,
    location_person_excess_total: u32,
    /// Distinct `Room.location` values across the whole tenant. `1` when
    /// `MinimizeLocationChange` is not configured — never `0`, so a `% `/
    /// index arithmetic against it never divides by zero.
    n_locations: usize,

    location_rules: Vec<MinimizeLocationChangeInstance>,

    /// `[room * n_slots + slot]` — per-slot occupancy count for exclusive
    /// Rooms, mirroring `group_slot`/`run_group_slot`/`span_group_slot` but
    /// keyed by Room rather than Group/Person. `RoomTurnaroundBuffer`'s own
    /// array, independent of the structural `RoomDoubleBooking` check
    /// (`Occupancy.room` in `solution.rs`), so the two remain independently
    /// switchable — exactly the reason `Compactness` does not reuse
    /// `group`/`attendee` either.
    turnaround_room_slot: Vec<u32>,
    /// The TIGHTEST (LARGEST — a BIGGER buffer is MORE restrictive, the
    /// opposite direction from every CAP-style threshold above, which is
    /// tightest at its SMALLEST) `buffer_blocks` among every enabled
    /// instance. `0` (never triggers) when not configured.
    turnaround_buffer_blocks: u32,
    /// Running count of violating Room-adjacency boundaries, maintained as an
    /// exact delta on add/remove — see [`Self::turnaround_boundary_violations`]
    /// for why this cannot be a before/after row rescan the way
    /// `group_gap_total` is: a plain occupancy count cannot tell two
    /// back-to-back Sessions apart from one long one, so the check must use
    /// the CANDIDATE'S OWN known span as the reference point instead of
    /// inferring boundaries from the array alone.
    turnaround_violations_total: u32,
    turnaround_rules: Vec<RoomTurnaroundBufferInstance>,

    /// `[group * n_weeks * n_rooms + week * n_rooms + room]` — occurrence
    /// count of Sessions this Group has in each Room, per week. Mirrors
    /// `location_group_loc`, with WEEK/ROOM in place of DAY/LOCATION.
    churn_room: Vec<u32>,
    /// `[group * n_weeks + week]` — how many DISTINCT Rooms this Group's
    /// Sessions currently touch that week, maintained incrementally exactly
    /// like `location_group_distinct`.
    churn_distinct: Vec<u32>,
    /// The TIGHTEST (SMALLEST — a cap, like `span_group_threshold`, not a
    /// minimum-required-value like `turnaround_buffer_blocks`)
    /// `max_rooms_per_week` among every enabled instance. `u32::MAX` when
    /// not configured.
    churn_threshold: u32,
    /// Running sum of excess-over-threshold distinct Rooms over every
    /// currently occupied `(Group, week)` cell, maintained as a delta exactly
    /// like `location_group_excess_total`.
    churn_excess_total: u32,
    /// Rooms across the whole tenant — `churn_room`'s and
    /// `turnaround_room_slot`'s shared row stride. `1` when neither type is
    /// configured, never `0`.
    n_rooms: usize,
    churn_rules: Vec<MinimizeRoomChurnInstance>,

    /// `[offering * n_rooms + room]` — occurrence count of this Offering's
    /// currently-placed Sessions in each Room. `RoomConsistency`'s own
    /// histogram; the MODAL entry in one Offering's row is its "usual" Room.
    consistency_room: Vec<u32>,
    /// Running sum, over every Offering, of Sessions NOT in that Offering's
    /// current modal Room — `total_placed - modal_count` per Offering,
    /// maintained as a delta exactly like `group_gap_total`. Unlike the
    /// distinct-count types above, this needs a full row rescan on EVERY
    /// mutation (not only some), since removing the modal Room's own
    /// occurrence can hand the mode to a different Room entirely — but
    /// `n_rooms` is the same bounded width `location_group_loc`'s inner axis
    /// already accepts a rescan over.
    consistency_excess_total: u32,
    consistency_rules: Vec<RoomConsistencyInstance>,

    /// Row per Offering: `(PersonIdx, occurrence count)` pairs for every
    /// lecturer this Offering's currently-placed Sessions have ever used —
    /// `LecturerConsistency`'s own histogram. Only allocated (and only ever
    /// written to, at the call site) for Offerings with a genuine lecturer
    /// pool, so a fixed assignment costs this tracking nothing. A small,
    /// linearly-scanned row rather than a dense `offering * person` matrix
    /// like `consistency_room` uses for Rooms: bounded by the Offering's own
    /// candidate pool size, the same O(|chosen lecturers|)-not-O(|all
    /// persons|) trade-off `PreferenceModel::cost_for` already makes for a
    /// pool Offering (ADR-0026, "Lecturer-pool selection landed").
    lecturer_rows: Vec<Vec<(PersonIdx, u32)>>,
    /// Running sum, over every tracked Offering, of
    /// `max(0, distinct_lecturers - required_lecturer_count)` — maintained as
    /// an exact delta on every mutation, mirroring `consistency_excess_total`
    /// but keyed by lecturer identity rather than by a modal Room.
    lecturer_excess_total: u32,
    lecturer_consistency_rules: Vec<LecturerConsistencyInstance>,
}

impl Aggregates {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        n_groups: usize,
        n_days: usize,
        n_weeks: usize,
        rules: Vec<ShareInstance>,
        n_persons: usize,
        n_slots: usize,
        blocks_per_day: usize,
        compactness_rules: Vec<CompactnessInstance>,
        n_offerings: usize,
        weekly_cells: usize,
        distributed_rules: Vec<PatternAdherenceInstance>,
        block_rules: Vec<PatternAdherenceInstance>,
        max_consecutive_rules: Vec<MaxConsecutiveInstance>,
        max_daily_span_rules: Vec<MaxDailySpanInstance>,
        daily_count_rules: Vec<MaxDailySessionCountInstance>,
        offering_daily_count_rules: Vec<MaxOfferingSessionsPerDayInstance>,
        offering_run_rules: Vec<MaxConsecutiveOfferingBlocksInstance>,
        offering_split_rules: Vec<MinimizeOfferingDaySplitInstance>,
        teaching_load_rules: Vec<MaxWeeklyTeachingLoadInstance>,
        exam_same_day_rules: Vec<ExamSpacingSameDayInstance>,
        exam_window_rules: Vec<ExamSpacingWindowInstance>,
        imbalance_rules: Vec<MinimizeWeekdayImbalanceInstance>,
        active_days_count: usize,
        max_days_rules: Vec<MaxDaysInstance>,
        max_consecutive_days_rules: Vec<MaxConsecutiveDaysInstance>,
        location_rules: Vec<MinimizeLocationChangeInstance>,
        n_locations: usize,
        turnaround_rules: Vec<RoomTurnaroundBufferInstance>,
        n_rooms: usize,
        churn_rules: Vec<MinimizeRoomChurnInstance>,
        consistency_rules: Vec<RoomConsistencyInstance>,
        lecturer_consistency_rules: Vec<LecturerConsistencyInstance>,
        daybreak_rules: Vec<DaybreakInstance>,
        travel_rules: Vec<TravelTimeInstance>,
    ) -> Self {
        let groups = n_groups.max(1);
        let counters = rules
            .iter()
            .map(|r| {
                let windows = match r.window {
                    ShareWindow::PerTerm => 1,
                    ShareWindow::PerWeek => n_weeks.max(1),
                };
                ShareCounters::new(groups, windows)
            })
            .collect();

        let track_group = compactness_rules.iter().any(|r| r.group);
        let track_person = compactness_rules.iter().any(|r| r.person);
        let slots = n_slots.max(1);
        let offerings = n_offerings.max(1);
        let cells = weekly_cells.max(1);
        let weeks = n_weeks.max(1);

        let track_run_group = max_consecutive_rules.iter().any(|r| r.group);
        let track_run_person = max_consecutive_rules.iter().any(|r| r.person);
        let run_group_threshold = max_consecutive_rules
            .iter()
            .filter(|r| r.group)
            .map(|r| r.max_consecutive)
            .min()
            .unwrap_or(u32::MAX);
        let run_person_threshold = max_consecutive_rules
            .iter()
            .filter(|r| r.person)
            .map(|r| r.max_consecutive)
            .min()
            .unwrap_or(u32::MAX);

        let track_span_group = max_daily_span_rules.iter().any(|r| r.group);
        let track_span_person = max_daily_span_rules.iter().any(|r| r.person);
        let span_group_threshold = max_daily_span_rules
            .iter()
            .filter(|r| r.group)
            .map(|r| r.max_span_blocks)
            .min()
            .unwrap_or(u32::MAX);
        let span_person_threshold = max_daily_span_rules
            .iter()
            .filter(|r| r.person)
            .map(|r| r.max_span_blocks)
            .min()
            .unwrap_or(u32::MAX);

        let track_daily_count_group = daily_count_rules.iter().any(|r| r.group);
        let track_daily_count_person = daily_count_rules.iter().any(|r| r.person);
        let daily_count_group_threshold = daily_count_rules
            .iter()
            .filter(|r| r.group)
            .map(|r| r.max_per_day)
            .min()
            .unwrap_or(u32::MAX);
        let daily_count_person_threshold = daily_count_rules
            .iter()
            .filter(|r| r.person)
            .map(|r| r.max_per_day)
            .min()
            .unwrap_or(u32::MAX);

        let track_offering_daily_count = !offering_daily_count_rules.is_empty();
        let offering_daily_count_threshold = offering_daily_count_rules
            .iter()
            .map(|r| r.max_per_day)
            .min()
            .unwrap_or(u32::MAX);

        let track_offering_run = !offering_run_rules.is_empty();
        let offering_run_threshold = offering_run_rules
            .iter()
            .map(|r| r.max_consecutive)
            .min()
            .unwrap_or(u32::MAX);

        let track_offering_split = !offering_split_rules.is_empty();

        let track_teaching_load = !teaching_load_rules.is_empty();
        // The instance supplying the tightest cap, since that is the one
        // "whichever binds hardest" reads off for both the threshold and its
        // own counting rule.
        let tightest_load_rule = teaching_load_rules.iter().min_by_key(|r| r.max_per_week);
        let teaching_load_threshold = tightest_load_rule.map_or(u32::MAX, |r| r.max_per_week);
        let teaching_load_count_blocks = tightest_load_rule.is_some_and(|r| r.count_blocks);

        let track_exam_same_day = !exam_same_day_rules.is_empty();
        let track_exam_window = !exam_window_rules.is_empty();
        let exam_window_threshold = exam_window_rules
            .iter()
            .map(|r| r.min_days_between)
            .min()
            .unwrap_or(u32::MAX);

        let track_imbalance = !imbalance_rules.is_empty();

        // Shared substrate: either type wanting an axis is enough to track
        // it, since both reduce the SAME day-occupancy cell.
        let track_day_cap_group = max_days_rules.iter().any(|r| r.group)
            || max_consecutive_days_rules.iter().any(|r| r.group);
        let track_day_cap_person = max_days_rules.iter().any(|r| r.person)
            || max_consecutive_days_rules.iter().any(|r| r.person);
        let max_days_group_threshold = max_days_rules
            .iter()
            .filter(|r| r.group)
            .map(|r| r.max_days)
            .min()
            .unwrap_or(u32::MAX);
        let max_days_person_threshold = max_days_rules
            .iter()
            .filter(|r| r.person)
            .map(|r| r.max_days)
            .min()
            .unwrap_or(u32::MAX);
        let max_consecutive_days_group_threshold = max_consecutive_days_rules
            .iter()
            .filter(|r| r.group)
            .map(|r| r.max_consecutive_days)
            .min()
            .unwrap_or(u32::MAX);
        let max_consecutive_days_person_threshold = max_consecutive_days_rules
            .iter()
            .filter(|r| r.person)
            .map(|r| r.max_consecutive_days)
            .min()
            .unwrap_or(u32::MAX);

        let track_daybreak_group = daybreak_rules.iter().any(|r| r.group);
        let track_daybreak_person = daybreak_rules.iter().any(|r| r.person);
        // A FLOOR, not a cap: the hardest-to-satisfy requirement is the
        // LONGEST rest demanded, so this is `max`, not `min` like every
        // cap-threshold above. `0` (never binding) when nothing configures
        // that axis.
        let daybreak_group_threshold_minutes = daybreak_rules
            .iter()
            .filter(|r| r.group)
            .map(|r| r.min_rest_minutes)
            .max()
            .unwrap_or(0);
        let daybreak_person_threshold_minutes = daybreak_rules
            .iter()
            .filter(|r| r.person)
            .map(|r| r.min_rest_minutes)
            .max()
            .unwrap_or(0);

        let track_travel_group = travel_rules.iter().any(|r| r.group);
        let track_travel_person = travel_rules.iter().any(|r| r.person);
        let travel_group_threshold_minutes = travel_rules
            .iter()
            .filter(|r| r.group)
            .map(|r| r.min_minutes_between_sites)
            .max()
            .unwrap_or(0);
        let travel_person_threshold_minutes = travel_rules
            .iter()
            .filter(|r| r.person)
            .map(|r| r.min_minutes_between_sites)
            .max()
            .unwrap_or(0);

        let track_location_group = location_rules.iter().any(|r| r.group);
        let track_location_person = location_rules.iter().any(|r| r.person);
        let location_group_threshold = location_rules
            .iter()
            .filter(|r| r.group)
            .map(|r| r.max_locations_per_day)
            .min()
            .unwrap_or(u32::MAX);
        let location_person_threshold = location_rules
            .iter()
            .filter(|r| r.person)
            .map(|r| r.max_locations_per_day)
            .min()
            .unwrap_or(u32::MAX);
        let locations = n_locations.max(1);

        let track_turnaround = !turnaround_rules.is_empty();
        let turnaround_buffer_blocks = turnaround_rules
            .iter()
            .map(|r| r.buffer_blocks)
            .max()
            .unwrap_or(0);
        let rooms = n_rooms.max(1);

        let track_churn = !churn_rules.is_empty();
        let churn_threshold = churn_rules
            .iter()
            .map(|r| r.max_rooms_per_week)
            .min()
            .unwrap_or(u32::MAX);

        let track_consistency = !consistency_rules.is_empty();
        let track_lecturer_consistency = !lecturer_consistency_rules.is_empty();

        Self {
            online_day: vec![0; groups * n_days.max(1)],
            onsite_day: vec![0; groups * n_days.max(1)],
            n_days: n_days.max(1),
            rules,
            counters,
            group_slot: if track_group { vec![0; groups * slots] } else { Vec::new() },
            person_slot: if track_person { vec![0; n_persons.max(1) * slots] } else { Vec::new() },
            group_gap_total: 0,
            person_gap_total: 0,
            n_slots: slots,
            blocks_per_day: blocks_per_day.max(1),
            compactness_rules,
            distributed_cell: if distributed_rules.is_empty() {
                Vec::new()
            } else {
                vec![0; offerings * cells]
            },
            distributed_nonzero: if distributed_rules.is_empty() {
                Vec::new()
            } else {
                vec![0; offerings]
            },
            distributed_total: 0,
            weekly_cells: cells,
            block_week: if block_rules.is_empty() {
                Vec::new()
            } else {
                vec![0; offerings * weeks]
            },
            block_gap_total: 0,
            n_weeks: weeks,
            distributed_rules,
            block_rules,
            run_group_slot: if track_run_group { vec![0; groups * slots] } else { Vec::new() },
            run_person_slot: if track_run_person {
                vec![0; n_persons.max(1) * slots]
            } else {
                Vec::new()
            },
            run_group_threshold,
            run_person_threshold,
            run_group_excess_total: 0,
            run_person_excess_total: 0,
            max_consecutive_rules,
            span_group_slot: if track_span_group { vec![0; groups * slots] } else { Vec::new() },
            span_person_slot: if track_span_person {
                vec![0; n_persons.max(1) * slots]
            } else {
                Vec::new()
            },
            span_group_threshold,
            span_person_threshold,
            span_group_excess_total: 0,
            span_person_excess_total: 0,
            max_daily_span_rules,
            daily_count_group: if track_daily_count_group {
                vec![0; groups * n_days.max(1)]
            } else {
                Vec::new()
            },
            daily_count_person: if track_daily_count_person {
                vec![0; n_persons.max(1) * n_days.max(1)]
            } else {
                Vec::new()
            },
            daily_count_group_threshold,
            daily_count_person_threshold,
            daily_count_group_excess_total: 0,
            daily_count_person_excess_total: 0,
            daily_count_rules,
            offering_daily_count: if track_offering_daily_count {
                vec![0; offerings * n_days.max(1)]
            } else {
                Vec::new()
            },
            offering_daily_count_threshold,
            offering_daily_count_excess_total: 0,
            offering_daily_count_rules,
            offering_run_slot: if track_offering_run {
                vec![0; offerings * slots]
            } else {
                Vec::new()
            },
            offering_run_threshold,
            offering_run_excess_total: 0,
            offering_run_rules,
            offering_split_slot: if track_offering_split {
                vec![0; offerings * slots]
            } else {
                Vec::new()
            },
            offering_split_excess_total: 0,
            offering_split_rules,
            teaching_load_week: if track_teaching_load {
                vec![0; n_persons.max(1) * weeks]
            } else {
                Vec::new()
            },
            teaching_load_threshold,
            teaching_load_count_blocks,
            teaching_load_excess_total: 0,
            teaching_load_rules,
            exam_same_day: if track_exam_same_day {
                vec![0; groups * n_days.max(1)]
            } else {
                Vec::new()
            },
            exam_same_day_rules,
            exam_window: if track_exam_window {
                vec![0; groups * n_days.max(1)]
            } else {
                Vec::new()
            },
            exam_window_threshold,
            exam_window_rules,
            imbalance_day: if track_imbalance {
                vec![0; groups * n_days.max(1)]
            } else {
                Vec::new()
            },
            active_days_count: active_days_count.max(1),
            imbalance_rules,
            day_cap_group: if track_day_cap_group {
                vec![0; groups * n_days.max(1)]
            } else {
                Vec::new()
            },
            day_cap_person: if track_day_cap_person {
                vec![0; n_persons.max(1) * n_days.max(1)]
            } else {
                Vec::new()
            },
            max_days_group_threshold,
            max_days_person_threshold,
            max_consecutive_days_group_threshold,
            max_consecutive_days_person_threshold,
            max_days_rules,
            max_consecutive_days_rules,
            daybreak_group_slot: if track_daybreak_group {
                vec![0; groups * slots]
            } else {
                Vec::new()
            },
            daybreak_person_slot: if track_daybreak_person {
                vec![0; n_persons.max(1) * slots]
            } else {
                Vec::new()
            },
            daybreak_group_threshold_minutes,
            daybreak_person_threshold_minutes,
            daybreak_rules,
            travel_group_slot: if track_travel_group {
                vec![u32::MAX; groups * slots]
            } else {
                Vec::new()
            },
            travel_person_slot: if track_travel_person {
                vec![u32::MAX; n_persons.max(1) * slots]
            } else {
                Vec::new()
            },
            travel_group_threshold_minutes,
            travel_person_threshold_minutes,
            travel_rules,
            location_group_loc: if track_location_group {
                vec![0; groups * n_days.max(1) * locations]
            } else {
                Vec::new()
            },
            location_person_loc: if track_location_person {
                vec![0; n_persons.max(1) * n_days.max(1) * locations]
            } else {
                Vec::new()
            },
            location_group_distinct: if track_location_group {
                vec![0; groups * n_days.max(1)]
            } else {
                Vec::new()
            },
            location_person_distinct: if track_location_person {
                vec![0; n_persons.max(1) * n_days.max(1)]
            } else {
                Vec::new()
            },
            location_group_threshold,
            location_person_threshold,
            location_group_excess_total: 0,
            location_person_excess_total: 0,
            n_locations: locations,
            location_rules,
            turnaround_room_slot: if track_turnaround {
                vec![0; rooms * slots]
            } else {
                Vec::new()
            },
            turnaround_buffer_blocks,
            turnaround_violations_total: 0,
            turnaround_rules,
            churn_room: if track_churn { vec![0; groups * weeks * rooms] } else { Vec::new() },
            churn_distinct: if track_churn { vec![0; groups * weeks] } else { Vec::new() },
            churn_threshold,
            churn_excess_total: 0,
            n_rooms: rooms,
            churn_rules,
            consistency_room: if track_consistency {
                vec![0; offerings * rooms]
            } else {
                Vec::new()
            },
            consistency_excess_total: 0,
            consistency_rules,
            lecturer_rows: if track_lecturer_consistency {
                vec![Vec::new(); offerings]
            } else {
                Vec::new()
            },
            lecturer_excess_total: 0,
            lecturer_consistency_rules,
        }
    }

    pub fn has_day_mix_state(&self) -> bool {
        self.n_days > 0
    }

    pub fn rules(&self) -> &[ShareInstance] {
        &self.rules
    }

    /// Total violated `(rule, group, window)` cells. This is the number that
    /// joins the objective's hard component.
    pub fn share_violations(&self) -> u32 {
        self.counters.iter().map(|c| c.violated).sum()
    }

    // -- day mix -----------------------------------------------------------

    #[inline]
    fn day_cell(&self, group: GroupIdx, day: u32) -> usize {
        group.get() * self.n_days + day as usize
    }

    /// Would placing a Session of these Groups, in this mode, on these days,
    /// create a mixed day?
    pub fn day_mix_allows(&self, groups: &[GroupIdx], days: &[u32], is_online: bool) -> bool {
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                let blocked =
                    if is_online { self.onsite_day[c] > 0 } else { self.online_day[c] > 0 };
                if blocked {
                    return false;
                }
            }
        }
        true
    }

    pub fn add_day_mode(&mut self, groups: &[GroupIdx], days: &[u32], is_online: bool) {
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                if is_online {
                    self.online_day[c] += 1;
                } else {
                    self.onsite_day[c] += 1;
                }
            }
        }
    }

    pub fn remove_day_mode(&mut self, groups: &[GroupIdx], days: &[u32], is_online: bool) {
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                if is_online {
                    self.online_day[c] = self.online_day[c].saturating_sub(1);
                } else {
                    self.onsite_day[c] = self.onsite_day[c].saturating_sub(1);
                }
            }
        }
    }

    /// Groups whose day is currently mixed, for diagnostics.
    /// How many `(group, day)` cells currently mix the two delivery modes.
    ///
    /// The number the objective charges for, and the counterpart of
    /// [`Self::share_violations`] — read straight off the counters rather than
    /// accumulated as a delta, because a mixed day is a property of a cell and
    /// not of any one placement. Two Sessions make a cell mixed; neither of them
    /// individually "costs" anything.
    pub fn day_mix_violations(&self) -> u32 {
        (0..self.online_day.len())
            .filter(|&c| self.online_day[c] > 0 && self.onsite_day[c] > 0)
            .count() as u32
    }

    /// Total `(group, day)` cells — the exact upper bound on how many can be
    /// mixed at once, which is what bounds the day-mix term's contribution to
    /// the objective. See `Problem::hard_penalty`.
    pub fn day_mix_cell_count(&self) -> usize {
        self.online_day.len()
    }

    pub fn mixed_days(&self) -> impl Iterator<Item = (GroupIdx, u32)> + '_ {
        (0..self.online_day.len())
            .filter(move |&c| self.online_day[c] > 0 && self.onsite_day[c] > 0)
            .map(move |c| (GroupIdx((c / self.n_days) as u32), (c % self.n_days) as u32))
    }

    // -- compactness ---------------------------------------------------------

    pub fn compactness_rules(&self) -> &[CompactnessInstance] {
        &self.compactness_rules
    }

    #[inline]
    fn day_range(&self, day: u32) -> (usize, usize) {
        let start = day as usize * self.blocks_per_day;
        (start, start + self.blocks_per_day)
    }

    /// Idle blocks strictly between the first and last occupied block of one
    /// day, for a row already narrowed to that day's `blocks_per_day` cells.
    /// `0` for an empty or single-block day: there is nothing "between" one
    /// occupied block, or none.
    #[inline]
    fn gap_u32(day_cells: &[u32]) -> u32 {
        let mut first = None;
        let mut last = 0usize;
        let mut occupied = 0u32;
        for (i, &c) in day_cells.iter().enumerate() {
            if c > 0 {
                first.get_or_insert(i);
                last = i;
                occupied += 1;
            }
        }
        match first {
            Some(f) => (last - f + 1) as u32 - occupied,
            None => 0,
        }
    }

    /// See [`Self::gap_u32`]. Duplicated rather than made generic: `person_slot`
    /// is `u8` specifically to halve-then-some its footprint at
    /// large-university scale, and a generic `PartialOrd + Default` version
    /// would cost the compiler nothing but would cost a reader the concrete
    /// type at the point that actually matters here.
    #[inline]
    fn gap_u8(day_cells: &[u8]) -> u32 {
        let mut first = None;
        let mut last = 0usize;
        let mut occupied = 0u32;
        for (i, &c) in day_cells.iter().enumerate() {
            if c > 0 {
                first.get_or_insert(i);
                last = i;
                occupied += 1;
            }
        }
        match first {
            Some(f) => (last - f + 1) as u32 - occupied,
            None => 0,
        }
    }

    /// `gap_u32`, but with `span` treated as already occupied — WITHOUT
    /// mutating `day_cells` or allocating, so `evaluator::score_one` can rank
    /// a candidate against this before it is chosen. `span` is a handful of
    /// slots (a Session's duration), so the linear `contains` scan per cell
    /// costs nothing a hot loop would notice.
    #[inline]
    fn gap_u32_with(day_cells: &[u32], start: usize, span: &[SlotIdx]) -> u32 {
        let mut first = None;
        let mut last = 0usize;
        let mut occupied = 0u32;
        for (i, &c) in day_cells.iter().enumerate() {
            let is_occupied = c > 0 || span.iter().any(|s| s.get() == start + i);
            if is_occupied {
                first.get_or_insert(i);
                last = i;
                occupied += 1;
            }
        }
        match first {
            Some(f) => (last - f + 1) as u32 - occupied,
            None => 0,
        }
    }

    /// See [`Self::gap_u32_with`]; the `u8` counterpart.
    #[inline]
    fn gap_u8_with(day_cells: &[u8], start: usize, span: &[SlotIdx]) -> u32 {
        let mut first = None;
        let mut last = 0usize;
        let mut occupied = 0u32;
        for (i, &c) in day_cells.iter().enumerate() {
            let is_occupied = c > 0 || span.iter().any(|s| s.get() == start + i);
            if is_occupied {
                first.get_or_insert(i);
                last = i;
                occupied += 1;
            }
        }
        match first {
            Some(f) => (last - f + 1) as u32 - occupied,
            None => 0,
        }
    }

    /// The gap DELTA compactness would experience if `groups` gained
    /// occupancy at `span`, without mutating anything — for ranking a
    /// candidate during repair before it is committed. `add_group_compactness`
    /// computes the same delta for real once a candidate is actually chosen;
    /// this is its read-only preview.
    pub fn group_compactness_delta(&self, groups: &[GroupIdx], day: u32, span: &[SlotIdx]) -> i64 {
        if self.group_slot.is_empty() || span.is_empty() {
            return 0;
        }
        let (start, end) = self.day_range(day);
        let mut delta = 0i64;
        for &g in groups {
            let row = g.get() * self.n_slots;
            let before = Self::gap_u32(&self.group_slot[row + start..row + end]);
            let after = Self::gap_u32_with(&self.group_slot[row + start..row + end], start, span);
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    /// See [`Self::group_compactness_delta`]; the Person counterpart.
    pub fn person_compactness_delta(
        &self,
        persons: &[PersonIdx],
        day: u32,
        span: &[SlotIdx],
    ) -> i64 {
        if self.person_slot.is_empty() || span.is_empty() {
            return 0;
        }
        let (start, end) = self.day_range(day);
        let mut delta = 0i64;
        for &p in persons {
            let row = p.get() * self.n_slots;
            let before = Self::gap_u8(&self.person_slot[row + start..row + end]);
            let after = Self::gap_u8_with(&self.person_slot[row + start..row + end], start, span);
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    /// Mark `span` busy for `groups`' compactness tracking, updating
    /// `group_gap_total` by the exact delta for each touched Group's day. A
    /// no-op when no configured `Compactness` covers the Group axis.
    ///
    /// `span` is a single Session's slots, always within one calendar day (see
    /// `SlotTable::span`), so exactly one day is touched per Group here — no
    /// loop over multiple days, unlike the day-mix counters above, which are
    /// handed every day a multi-day-spanning CALLER context might cover.
    pub fn add_group_compactness(&mut self, groups: &[GroupIdx], day: u32, span: &[SlotIdx]) {
        if self.group_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &g in groups {
            let row = g.get() * self.n_slots;
            let before = Self::gap_u32(&self.group_slot[row + start..row + end]);
            for &s in span {
                self.group_slot[row + s.get()] += 1;
            }
            let after = Self::gap_u32(&self.group_slot[row + start..row + end]);
            self.group_gap_total =
                (i64::from(self.group_gap_total) + i64::from(after) - i64::from(before)) as u32;
        }
    }

    pub fn remove_group_compactness(&mut self, groups: &[GroupIdx], day: u32, span: &[SlotIdx]) {
        if self.group_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &g in groups {
            let row = g.get() * self.n_slots;
            let before = Self::gap_u32(&self.group_slot[row + start..row + end]);
            for &s in span {
                self.group_slot[row + s.get()] = self.group_slot[row + s.get()].saturating_sub(1);
            }
            let after = Self::gap_u32(&self.group_slot[row + start..row + end]);
            self.group_gap_total =
                (i64::from(self.group_gap_total) + i64::from(after) - i64::from(before)) as u32;
        }
    }

    /// See [`Self::add_group_compactness`]; the Person counterpart, over
    /// `who.attendees` rather than `who.subtree_groups`.
    pub fn add_person_compactness(&mut self, persons: &[PersonIdx], day: u32, span: &[SlotIdx]) {
        if self.person_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &p in persons {
            let row = p.get() * self.n_slots;
            let before = Self::gap_u8(&self.person_slot[row + start..row + end]);
            for &s in span {
                let c = row + s.get();
                self.person_slot[c] = self.person_slot[c].saturating_add(1);
            }
            let after = Self::gap_u8(&self.person_slot[row + start..row + end]);
            self.person_gap_total =
                (i64::from(self.person_gap_total) + i64::from(after) - i64::from(before)) as u32;
        }
    }

    pub fn remove_person_compactness(&mut self, persons: &[PersonIdx], day: u32, span: &[SlotIdx]) {
        if self.person_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &p in persons {
            let row = p.get() * self.n_slots;
            let before = Self::gap_u8(&self.person_slot[row + start..row + end]);
            for &s in span {
                let c = row + s.get();
                self.person_slot[c] = self.person_slot[c].saturating_sub(1);
            }
            let after = Self::gap_u8(&self.person_slot[row + start..row + end]);
            self.person_gap_total =
                (i64::from(self.person_gap_total) + i64::from(after) - i64::from(before)) as u32;
        }
    }

    /// What the currently idle blocks cost, at the configured weight — O(1),
    /// read straight off `group_gap_total`/`person_gap_total` rather than
    /// rescanned, for the reason those fields' own doc gives.
    pub fn compactness_cost(&self, group_weight: f64, person_weight: f64) -> f64 {
        self.group_gap_total as f64 * group_weight + self.person_gap_total as f64 * person_weight
    }

    /// Sum of `weight` over every currently-gapped `(entity, day)` cell this
    /// occupant participates in, for `ruin_worst`'s attribution.
    ///
    /// Like `day_mix_violation_cost`, every occupant of a gapped cell is
    /// charged, not only the ones at the gap's edges: removing ANY Session on
    /// a gapped day can only shrink the occupied range or leave it the same,
    /// never make a NEW gap where none existed, so there is no occupant in a
    /// gapped cell whose removal is provably useless the way an on-site
    /// Session is in a share breach.
    pub fn compactness_ruin_cost(
        &self,
        groups: &[GroupIdx],
        persons: &[PersonIdx],
        day: u32,
        group_weight: f64,
        person_weight: f64,
    ) -> f64 {
        let mut cost = 0.0;
        if group_weight != 0.0 && !self.group_slot.is_empty() {
            let (start, end) = self.day_range(day);
            for &g in groups {
                let row = g.get() * self.n_slots;
                if Self::gap_u32(&self.group_slot[row + start..row + end]) > 0 {
                    cost += group_weight;
                }
            }
        }
        if person_weight != 0.0 && !self.person_slot.is_empty() {
            let (start, end) = self.day_range(day);
            for &p in persons {
                let row = p.get() * self.n_slots;
                if Self::gap_u8(&self.person_slot[row + start..row + end]) > 0 {
                    cost += person_weight;
                }
            }
        }
        cost
    }

    // -- max consecutive blocks -----------------------------------------------

    pub fn max_consecutive_rules(&self) -> &[MaxConsecutiveInstance] {
        &self.max_consecutive_rules
    }

    /// Blocks charged over a run longer than `threshold`, for a row already
    /// narrowed to one day's `blocks_per_day` cells — the mirror of
    /// `gap_u32`: instead of idle blocks between the ends, this sums, over
    /// every maximal run of CONSECUTIVE occupied blocks, how far past
    /// `threshold` that run reaches. Zero when every run fits.
    #[inline]
    fn run_excess_u32(day_cells: &[u32], threshold: u32) -> u32 {
        let mut total = 0u32;
        let mut run = 0u32;
        for &c in day_cells {
            if c > 0 {
                run += 1;
            } else {
                total += run.saturating_sub(threshold);
                run = 0;
            }
        }
        total + run.saturating_sub(threshold)
    }

    /// See [`Self::run_excess_u32`]; the `u8` counterpart, for the same
    /// footprint reason [`Self::gap_u8`] exists.
    #[inline]
    fn run_excess_u8(day_cells: &[u8], threshold: u32) -> u32 {
        let mut total = 0u32;
        let mut run = 0u32;
        for &c in day_cells {
            if c > 0 {
                run += 1;
            } else {
                total += run.saturating_sub(threshold);
                run = 0;
            }
        }
        total + run.saturating_sub(threshold)
    }

    /// `run_excess_u32`, but with `span` treated as already occupied —
    /// WITHOUT mutating or allocating, mirroring [`Self::gap_u32_with`].
    #[inline]
    fn run_excess_u32_with(
        day_cells: &[u32],
        start: usize,
        span: &[SlotIdx],
        threshold: u32,
    ) -> u32 {
        let mut total = 0u32;
        let mut run = 0u32;
        for (i, &c) in day_cells.iter().enumerate() {
            let is_occupied = c > 0 || span.iter().any(|s| s.get() == start + i);
            if is_occupied {
                run += 1;
            } else {
                total += run.saturating_sub(threshold);
                run = 0;
            }
        }
        total + run.saturating_sub(threshold)
    }

    /// See [`Self::run_excess_u32_with`]; the `u8` counterpart.
    #[inline]
    fn run_excess_u8_with(day_cells: &[u8], start: usize, span: &[SlotIdx], threshold: u32) -> u32 {
        let mut total = 0u32;
        let mut run = 0u32;
        for (i, &c) in day_cells.iter().enumerate() {
            let is_occupied = c > 0 || span.iter().any(|s| s.get() == start + i);
            if is_occupied {
                run += 1;
            } else {
                total += run.saturating_sub(threshold);
                run = 0;
            }
        }
        total + run.saturating_sub(threshold)
    }

    /// The run-excess DELTA `MaxConsecutiveBlocks` would experience if
    /// `groups` gained occupancy at `span` — the read-only preview
    /// `evaluator::score_one` ranks candidates against, mirroring
    /// [`Self::group_compactness_delta`].
    pub fn group_run_delta(&self, groups: &[GroupIdx], day: u32, span: &[SlotIdx]) -> i64 {
        if self.run_group_slot.is_empty() || span.is_empty() {
            return 0;
        }
        let (start, end) = self.day_range(day);
        let mut delta = 0i64;
        for &g in groups {
            let row = g.get() * self.n_slots;
            let before = Self::run_excess_u32(
                &self.run_group_slot[row + start..row + end],
                self.run_group_threshold,
            );
            let after = Self::run_excess_u32_with(
                &self.run_group_slot[row + start..row + end],
                start,
                span,
                self.run_group_threshold,
            );
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    /// See [`Self::group_run_delta`]; the Person counterpart.
    pub fn person_run_delta(&self, persons: &[PersonIdx], day: u32, span: &[SlotIdx]) -> i64 {
        if self.run_person_slot.is_empty() || span.is_empty() {
            return 0;
        }
        let (start, end) = self.day_range(day);
        let mut delta = 0i64;
        for &p in persons {
            let row = p.get() * self.n_slots;
            let before = Self::run_excess_u8(
                &self.run_person_slot[row + start..row + end],
                self.run_person_threshold,
            );
            let after = Self::run_excess_u8_with(
                &self.run_person_slot[row + start..row + end],
                start,
                span,
                self.run_person_threshold,
            );
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    pub fn add_group_run(&mut self, groups: &[GroupIdx], day: u32, span: &[SlotIdx]) {
        if self.run_group_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &g in groups {
            let row = g.get() * self.n_slots;
            let before = Self::run_excess_u32(
                &self.run_group_slot[row + start..row + end],
                self.run_group_threshold,
            );
            for &s in span {
                self.run_group_slot[row + s.get()] += 1;
            }
            let after = Self::run_excess_u32(
                &self.run_group_slot[row + start..row + end],
                self.run_group_threshold,
            );
            self.run_group_excess_total = (i64::from(self.run_group_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn remove_group_run(&mut self, groups: &[GroupIdx], day: u32, span: &[SlotIdx]) {
        if self.run_group_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &g in groups {
            let row = g.get() * self.n_slots;
            let before = Self::run_excess_u32(
                &self.run_group_slot[row + start..row + end],
                self.run_group_threshold,
            );
            for &s in span {
                self.run_group_slot[row + s.get()] =
                    self.run_group_slot[row + s.get()].saturating_sub(1);
            }
            let after = Self::run_excess_u32(
                &self.run_group_slot[row + start..row + end],
                self.run_group_threshold,
            );
            self.run_group_excess_total = (i64::from(self.run_group_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn add_person_run(&mut self, persons: &[PersonIdx], day: u32, span: &[SlotIdx]) {
        if self.run_person_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &p in persons {
            let row = p.get() * self.n_slots;
            let before = Self::run_excess_u8(
                &self.run_person_slot[row + start..row + end],
                self.run_person_threshold,
            );
            for &s in span {
                let c = row + s.get();
                self.run_person_slot[c] = self.run_person_slot[c].saturating_add(1);
            }
            let after = Self::run_excess_u8(
                &self.run_person_slot[row + start..row + end],
                self.run_person_threshold,
            );
            self.run_person_excess_total = (i64::from(self.run_person_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn remove_person_run(&mut self, persons: &[PersonIdx], day: u32, span: &[SlotIdx]) {
        if self.run_person_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &p in persons {
            let row = p.get() * self.n_slots;
            let before = Self::run_excess_u8(
                &self.run_person_slot[row + start..row + end],
                self.run_person_threshold,
            );
            for &s in span {
                let c = row + s.get();
                self.run_person_slot[c] = self.run_person_slot[c].saturating_sub(1);
            }
            let after = Self::run_excess_u8(
                &self.run_person_slot[row + start..row + end],
                self.run_person_threshold,
            );
            self.run_person_excess_total = (i64::from(self.run_person_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    /// What the currently over-cap runs cost, at the configured weight(s) —
    /// O(1), read straight off the running totals. Mirrors
    /// [`Self::compactness_cost`].
    pub fn max_consecutive_cost(&self, group_weight: f64, person_weight: f64) -> f64 {
        self.run_group_excess_total as f64 * group_weight
            + self.run_person_excess_total as f64 * person_weight
    }

    /// Sum of `weight` over every currently over-cap `(entity, day)` cell
    /// this occupant participates in, for `ruin_worst`'s attribution.
    /// Mirrors [`Self::compactness_ruin_cost`].
    pub fn max_consecutive_ruin_cost(
        &self,
        groups: &[GroupIdx],
        persons: &[PersonIdx],
        day: u32,
        group_weight: f64,
        person_weight: f64,
    ) -> f64 {
        let mut cost = 0.0;
        if group_weight != 0.0 && !self.run_group_slot.is_empty() {
            let (start, end) = self.day_range(day);
            for &g in groups {
                let row = g.get() * self.n_slots;
                if Self::run_excess_u32(
                    &self.run_group_slot[row + start..row + end],
                    self.run_group_threshold,
                ) > 0
                {
                    cost += group_weight;
                }
            }
        }
        if person_weight != 0.0 && !self.run_person_slot.is_empty() {
            let (start, end) = self.day_range(day);
            for &p in persons {
                let row = p.get() * self.n_slots;
                if Self::run_excess_u8(
                    &self.run_person_slot[row + start..row + end],
                    self.run_person_threshold,
                ) > 0
                {
                    cost += person_weight;
                }
            }
        }
        cost
    }

    // -- max daily span --------------------------------------------------------

    pub fn max_daily_span_rules(&self) -> &[MaxDailySpanInstance] {
        &self.max_daily_span_rules
    }

    /// Blocks charged over the elapsed span from first to last occupied
    /// block of one day, past `threshold` — the simpler cousin of
    /// `run_excess_u32`: a day has exactly one span (unlike possibly several
    /// runs), so there is nothing to sum over multiple stretches.
    #[inline]
    fn span_excess_u32(day_cells: &[u32], threshold: u32) -> u32 {
        let mut first = None;
        let mut last = 0usize;
        for (i, &c) in day_cells.iter().enumerate() {
            if c > 0 {
                first.get_or_insert(i);
                last = i;
            }
        }
        match first {
            Some(f) => ((last - f + 1) as u32).saturating_sub(threshold),
            None => 0,
        }
    }

    /// See [`Self::span_excess_u32`]; the `u8` counterpart.
    #[inline]
    fn span_excess_u8(day_cells: &[u8], threshold: u32) -> u32 {
        let mut first = None;
        let mut last = 0usize;
        for (i, &c) in day_cells.iter().enumerate() {
            if c > 0 {
                first.get_or_insert(i);
                last = i;
            }
        }
        match first {
            Some(f) => ((last - f + 1) as u32).saturating_sub(threshold),
            None => 0,
        }
    }

    /// `span_excess_u32`, but with `span` treated as already occupied —
    /// WITHOUT mutating or allocating, mirroring [`Self::gap_u32_with`].
    #[inline]
    fn span_excess_u32_with(
        day_cells: &[u32],
        start: usize,
        span: &[SlotIdx],
        threshold: u32,
    ) -> u32 {
        let mut first = None;
        let mut last = 0usize;
        for (i, &c) in day_cells.iter().enumerate() {
            if c > 0 || span.iter().any(|s| s.get() == start + i) {
                first.get_or_insert(i);
                last = i;
            }
        }
        match first {
            Some(f) => ((last - f + 1) as u32).saturating_sub(threshold),
            None => 0,
        }
    }

    /// See [`Self::span_excess_u32_with`]; the `u8` counterpart.
    #[inline]
    fn span_excess_u8_with(
        day_cells: &[u8],
        start: usize,
        span: &[SlotIdx],
        threshold: u32,
    ) -> u32 {
        let mut first = None;
        let mut last = 0usize;
        for (i, &c) in day_cells.iter().enumerate() {
            if c > 0 || span.iter().any(|s| s.get() == start + i) {
                first.get_or_insert(i);
                last = i;
            }
        }
        match first {
            Some(f) => ((last - f + 1) as u32).saturating_sub(threshold),
            None => 0,
        }
    }

    /// The span-excess DELTA `MaxDailySpan` would experience if `groups`
    /// gained occupancy at `span` — the read-only preview, mirroring
    /// [`Self::group_run_delta`].
    pub fn group_span_delta(&self, groups: &[GroupIdx], day: u32, span: &[SlotIdx]) -> i64 {
        if self.span_group_slot.is_empty() || span.is_empty() {
            return 0;
        }
        let (start, end) = self.day_range(day);
        let mut delta = 0i64;
        for &g in groups {
            let row = g.get() * self.n_slots;
            let before = Self::span_excess_u32(
                &self.span_group_slot[row + start..row + end],
                self.span_group_threshold,
            );
            let after = Self::span_excess_u32_with(
                &self.span_group_slot[row + start..row + end],
                start,
                span,
                self.span_group_threshold,
            );
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    /// See [`Self::group_span_delta`]; the Person counterpart.
    pub fn person_span_delta(&self, persons: &[PersonIdx], day: u32, span: &[SlotIdx]) -> i64 {
        if self.span_person_slot.is_empty() || span.is_empty() {
            return 0;
        }
        let (start, end) = self.day_range(day);
        let mut delta = 0i64;
        for &p in persons {
            let row = p.get() * self.n_slots;
            let before = Self::span_excess_u8(
                &self.span_person_slot[row + start..row + end],
                self.span_person_threshold,
            );
            let after = Self::span_excess_u8_with(
                &self.span_person_slot[row + start..row + end],
                start,
                span,
                self.span_person_threshold,
            );
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    pub fn add_group_span(&mut self, groups: &[GroupIdx], day: u32, span: &[SlotIdx]) {
        if self.span_group_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &g in groups {
            let row = g.get() * self.n_slots;
            let before = Self::span_excess_u32(
                &self.span_group_slot[row + start..row + end],
                self.span_group_threshold,
            );
            for &s in span {
                self.span_group_slot[row + s.get()] += 1;
            }
            let after = Self::span_excess_u32(
                &self.span_group_slot[row + start..row + end],
                self.span_group_threshold,
            );
            self.span_group_excess_total = (i64::from(self.span_group_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn remove_group_span(&mut self, groups: &[GroupIdx], day: u32, span: &[SlotIdx]) {
        if self.span_group_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &g in groups {
            let row = g.get() * self.n_slots;
            let before = Self::span_excess_u32(
                &self.span_group_slot[row + start..row + end],
                self.span_group_threshold,
            );
            for &s in span {
                self.span_group_slot[row + s.get()] =
                    self.span_group_slot[row + s.get()].saturating_sub(1);
            }
            let after = Self::span_excess_u32(
                &self.span_group_slot[row + start..row + end],
                self.span_group_threshold,
            );
            self.span_group_excess_total = (i64::from(self.span_group_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn add_person_span(&mut self, persons: &[PersonIdx], day: u32, span: &[SlotIdx]) {
        if self.span_person_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &p in persons {
            let row = p.get() * self.n_slots;
            let before = Self::span_excess_u8(
                &self.span_person_slot[row + start..row + end],
                self.span_person_threshold,
            );
            for &s in span {
                let c = row + s.get();
                self.span_person_slot[c] = self.span_person_slot[c].saturating_add(1);
            }
            let after = Self::span_excess_u8(
                &self.span_person_slot[row + start..row + end],
                self.span_person_threshold,
            );
            self.span_person_excess_total = (i64::from(self.span_person_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn remove_person_span(&mut self, persons: &[PersonIdx], day: u32, span: &[SlotIdx]) {
        if self.span_person_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        for &p in persons {
            let row = p.get() * self.n_slots;
            let before = Self::span_excess_u8(
                &self.span_person_slot[row + start..row + end],
                self.span_person_threshold,
            );
            for &s in span {
                let c = row + s.get();
                self.span_person_slot[c] = self.span_person_slot[c].saturating_sub(1);
            }
            let after = Self::span_excess_u8(
                &self.span_person_slot[row + start..row + end],
                self.span_person_threshold,
            );
            self.span_person_excess_total = (i64::from(self.span_person_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    /// What the currently over-cap daily spans cost, at the configured
    /// weight(s). Mirrors [`Self::max_consecutive_cost`].
    pub fn max_daily_span_cost(&self, group_weight: f64, person_weight: f64) -> f64 {
        self.span_group_excess_total as f64 * group_weight
            + self.span_person_excess_total as f64 * person_weight
    }

    /// Sum of `weight` over every currently over-cap `(entity, day)` cell
    /// this occupant participates in, for `ruin_worst`'s attribution.
    /// Mirrors [`Self::max_consecutive_ruin_cost`].
    pub fn max_daily_span_ruin_cost(
        &self,
        groups: &[GroupIdx],
        persons: &[PersonIdx],
        day: u32,
        group_weight: f64,
        person_weight: f64,
    ) -> f64 {
        let mut cost = 0.0;
        if group_weight != 0.0 && !self.span_group_slot.is_empty() {
            let (start, end) = self.day_range(day);
            for &g in groups {
                let row = g.get() * self.n_slots;
                if Self::span_excess_u32(
                    &self.span_group_slot[row + start..row + end],
                    self.span_group_threshold,
                ) > 0
                {
                    cost += group_weight;
                }
            }
        }
        if person_weight != 0.0 && !self.span_person_slot.is_empty() {
            let (start, end) = self.day_range(day);
            for &p in persons {
                let row = p.get() * self.n_slots;
                if Self::span_excess_u8(
                    &self.span_person_slot[row + start..row + end],
                    self.span_person_threshold,
                ) > 0
                {
                    cost += person_weight;
                }
            }
        }
        cost
    }

    // -- max daily session count -------------------------------------------------

    pub fn daily_count_rules(&self) -> &[MaxDailySessionCountInstance] {
        &self.daily_count_rules
    }

    #[inline]
    fn daily_count_excess(count: u32, threshold: u32) -> u32 {
        count.saturating_sub(threshold)
    }

    /// The count-excess DELTA `MaxDailySessionCount` would experience if
    /// `groups` gained one more Session on `day` — the read-only preview,
    /// mirroring [`Self::group_span_delta`] but a plain +1 rather than a
    /// span/gap recompute, since a raw count needs no slot-level detail.
    pub fn group_daily_count_delta(&self, groups: &[GroupIdx], day: u32) -> i64 {
        if self.daily_count_group.is_empty() {
            return 0;
        }
        let mut delta = 0i64;
        for &g in groups {
            let c = g.get() * self.n_days + day as usize;
            let before = Self::daily_count_excess(
                self.daily_count_group[c],
                self.daily_count_group_threshold,
            );
            let after = Self::daily_count_excess(
                self.daily_count_group[c] + 1,
                self.daily_count_group_threshold,
            );
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    /// See [`Self::group_daily_count_delta`]; the Person counterpart.
    pub fn person_daily_count_delta(&self, persons: &[PersonIdx], day: u32) -> i64 {
        if self.daily_count_person.is_empty() {
            return 0;
        }
        let mut delta = 0i64;
        for &p in persons {
            let c = p.get() * self.n_days + day as usize;
            let before = Self::daily_count_excess(
                self.daily_count_person[c],
                self.daily_count_person_threshold,
            );
            let after = Self::daily_count_excess(
                self.daily_count_person[c] + 1,
                self.daily_count_person_threshold,
            );
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    pub fn add_group_daily_count(&mut self, groups: &[GroupIdx], day: u32) {
        if self.daily_count_group.is_empty() {
            return;
        }
        for &g in groups {
            let c = g.get() * self.n_days + day as usize;
            let before = Self::daily_count_excess(
                self.daily_count_group[c],
                self.daily_count_group_threshold,
            );
            self.daily_count_group[c] += 1;
            let after = Self::daily_count_excess(
                self.daily_count_group[c],
                self.daily_count_group_threshold,
            );
            self.daily_count_group_excess_total = (i64::from(self.daily_count_group_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn remove_group_daily_count(&mut self, groups: &[GroupIdx], day: u32) {
        if self.daily_count_group.is_empty() {
            return;
        }
        for &g in groups {
            let c = g.get() * self.n_days + day as usize;
            let before = Self::daily_count_excess(
                self.daily_count_group[c],
                self.daily_count_group_threshold,
            );
            self.daily_count_group[c] = self.daily_count_group[c].saturating_sub(1);
            let after = Self::daily_count_excess(
                self.daily_count_group[c],
                self.daily_count_group_threshold,
            );
            self.daily_count_group_excess_total = (i64::from(self.daily_count_group_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn add_person_daily_count(&mut self, persons: &[PersonIdx], day: u32) {
        if self.daily_count_person.is_empty() {
            return;
        }
        for &p in persons {
            let c = p.get() * self.n_days + day as usize;
            let before = Self::daily_count_excess(
                self.daily_count_person[c],
                self.daily_count_person_threshold,
            );
            self.daily_count_person[c] += 1;
            let after = Self::daily_count_excess(
                self.daily_count_person[c],
                self.daily_count_person_threshold,
            );
            self.daily_count_person_excess_total = (i64::from(self.daily_count_person_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn remove_person_daily_count(&mut self, persons: &[PersonIdx], day: u32) {
        if self.daily_count_person.is_empty() {
            return;
        }
        for &p in persons {
            let c = p.get() * self.n_days + day as usize;
            let before = Self::daily_count_excess(
                self.daily_count_person[c],
                self.daily_count_person_threshold,
            );
            self.daily_count_person[c] = self.daily_count_person[c].saturating_sub(1);
            let after = Self::daily_count_excess(
                self.daily_count_person[c],
                self.daily_count_person_threshold,
            );
            self.daily_count_person_excess_total = (i64::from(self.daily_count_person_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    /// What the currently over-cap daily Session counts cost, at the
    /// configured weight(s). Mirrors [`Self::max_daily_span_cost`].
    pub fn max_daily_session_count_cost(&self, group_weight: f64, person_weight: f64) -> f64 {
        self.daily_count_group_excess_total as f64 * group_weight
            + self.daily_count_person_excess_total as f64 * person_weight
    }

    /// Sum of `weight` over every currently over-cap `(entity, day)` cell
    /// this occupant participates in, for `ruin_worst`'s attribution. Mirrors
    /// [`Self::max_daily_span_ruin_cost`].
    pub fn max_daily_session_count_ruin_cost(
        &self,
        groups: &[GroupIdx],
        persons: &[PersonIdx],
        day: u32,
        group_weight: f64,
        person_weight: f64,
    ) -> f64 {
        let mut cost = 0.0;
        if group_weight != 0.0 && !self.daily_count_group.is_empty() {
            for &g in groups {
                let c = g.get() * self.n_days + day as usize;
                if Self::daily_count_excess(
                    self.daily_count_group[c],
                    self.daily_count_group_threshold,
                ) > 0
                {
                    cost += group_weight;
                }
            }
        }
        if person_weight != 0.0 && !self.daily_count_person.is_empty() {
            for &p in persons {
                let c = p.get() * self.n_days + day as usize;
                if Self::daily_count_excess(
                    self.daily_count_person[c],
                    self.daily_count_person_threshold,
                ) > 0
                {
                    cost += person_weight;
                }
            }
        }
        cost
    }

    // -- (offering, day) cluster: count / consecutive-run / split ---------------
    //
    // Issues #34 (MaxConsecutiveOfferingBlocks), #35 (MaxOfferingSessionsPerDay)
    // and #29 (MinimizeOfferingDaySplit) all reduce the same cell — one
    // Offering's occupied blocks within one day — three different ways. See
    // the doc comment on the instance structs for why storage stays separate
    // per type despite the shared cell.

    pub fn offering_daily_count_rules(&self) -> &[MaxOfferingSessionsPerDayInstance] {
        &self.offering_daily_count_rules
    }

    pub fn offering_run_rules(&self) -> &[MaxConsecutiveOfferingBlocksInstance] {
        &self.offering_run_rules
    }

    pub fn offering_split_rules(&self) -> &[MinimizeOfferingDaySplitInstance] {
        &self.offering_split_rules
    }

    /// The number of separate maximal runs of occupied blocks in one day's
    /// row — NOT their length (`run_excess_u32` already answers that). `0`
    /// for an entirely idle day, `1` for a single run however long,
    /// `MinimizeOfferingDaySplit`'s excess is this minus one, floored at
    /// zero by the caller.
    #[inline]
    fn run_count_u32(day_cells: &[u32]) -> u32 {
        let mut count = 0u32;
        let mut in_run = false;
        for &c in day_cells {
            if c > 0 {
                if !in_run {
                    count += 1;
                }
                in_run = true;
            } else {
                in_run = false;
            }
        }
        count
    }

    /// `run_count_u32`, but with `span` treated as already occupied —
    /// mirroring [`Self::run_excess_u32_with`].
    #[inline]
    fn run_count_u32_with(day_cells: &[u32], start: usize, span: &[SlotIdx]) -> u32 {
        let mut count = 0u32;
        let mut in_run = false;
        for (i, &c) in day_cells.iter().enumerate() {
            let occupied = c > 0 || span.iter().any(|s| s.get() == start + i);
            if occupied {
                if !in_run {
                    count += 1;
                }
                in_run = true;
            } else {
                in_run = false;
            }
        }
        count
    }

    /// The count-excess DELTA `MaxOfferingSessionsPerDay` would experience if
    /// `offering` gained one more Session on `day` — mirrors
    /// [`Self::group_daily_count_delta`], singular rather than a slice: an
    /// occupant realizes at most one Offering.
    pub fn offering_daily_count_delta(&self, offering: OfferingIdx, day: u32) -> i64 {
        if self.offering_daily_count.is_empty() {
            return 0;
        }
        let c = offering.get() * self.n_days + day as usize;
        let before = Self::daily_count_excess(
            self.offering_daily_count[c],
            self.offering_daily_count_threshold,
        );
        let after = Self::daily_count_excess(
            self.offering_daily_count[c] + 1,
            self.offering_daily_count_threshold,
        );
        i64::from(after) - i64::from(before)
    }

    pub fn add_offering_daily_count(&mut self, offering: OfferingIdx, day: u32) {
        if self.offering_daily_count.is_empty() {
            return;
        }
        let c = offering.get() * self.n_days + day as usize;
        let before = Self::daily_count_excess(
            self.offering_daily_count[c],
            self.offering_daily_count_threshold,
        );
        self.offering_daily_count[c] += 1;
        let after = Self::daily_count_excess(
            self.offering_daily_count[c],
            self.offering_daily_count_threshold,
        );
        self.offering_daily_count_excess_total = (i64::from(self.offering_daily_count_excess_total)
            + i64::from(after)
            - i64::from(before)) as u32;
    }

    pub fn remove_offering_daily_count(&mut self, offering: OfferingIdx, day: u32) {
        if self.offering_daily_count.is_empty() {
            return;
        }
        let c = offering.get() * self.n_days + day as usize;
        let before = Self::daily_count_excess(
            self.offering_daily_count[c],
            self.offering_daily_count_threshold,
        );
        self.offering_daily_count[c] = self.offering_daily_count[c].saturating_sub(1);
        let after = Self::daily_count_excess(
            self.offering_daily_count[c],
            self.offering_daily_count_threshold,
        );
        self.offering_daily_count_excess_total = (i64::from(self.offering_daily_count_excess_total)
            + i64::from(after)
            - i64::from(before)) as u32;
    }

    pub fn offering_daily_count_cost(&self, weight: f64) -> f64 {
        self.offering_daily_count_excess_total as f64 * weight
    }

    pub fn offering_daily_count_ruin_cost(
        &self,
        offering: OfferingIdx,
        day: u32,
        weight: f64,
    ) -> f64 {
        if weight == 0.0 || self.offering_daily_count.is_empty() {
            return 0.0;
        }
        let c = offering.get() * self.n_days + day as usize;
        if Self::daily_count_excess(
            self.offering_daily_count[c],
            self.offering_daily_count_threshold,
        ) > 0
        {
            weight
        } else {
            0.0
        }
    }

    /// The run-excess DELTA `MaxConsecutiveOfferingBlocks` would experience —
    /// mirrors [`Self::group_run_delta`], singular Offering.
    pub fn offering_run_delta(&self, offering: OfferingIdx, day: u32, span: &[SlotIdx]) -> i64 {
        if self.offering_run_slot.is_empty() || span.is_empty() {
            return 0;
        }
        let (start, end) = self.day_range(day);
        let row = offering.get() * self.n_slots;
        let before = Self::run_excess_u32(
            &self.offering_run_slot[row + start..row + end],
            self.offering_run_threshold,
        );
        let after = Self::run_excess_u32_with(
            &self.offering_run_slot[row + start..row + end],
            start,
            span,
            self.offering_run_threshold,
        );
        i64::from(after) - i64::from(before)
    }

    pub fn add_offering_run(&mut self, offering: OfferingIdx, day: u32, span: &[SlotIdx]) {
        if self.offering_run_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        let row = offering.get() * self.n_slots;
        let before = Self::run_excess_u32(
            &self.offering_run_slot[row + start..row + end],
            self.offering_run_threshold,
        );
        for &s in span {
            self.offering_run_slot[row + s.get()] += 1;
        }
        let after = Self::run_excess_u32(
            &self.offering_run_slot[row + start..row + end],
            self.offering_run_threshold,
        );
        self.offering_run_excess_total = (i64::from(self.offering_run_excess_total)
            + i64::from(after)
            - i64::from(before)) as u32;
    }

    pub fn remove_offering_run(&mut self, offering: OfferingIdx, day: u32, span: &[SlotIdx]) {
        if self.offering_run_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        let row = offering.get() * self.n_slots;
        let before = Self::run_excess_u32(
            &self.offering_run_slot[row + start..row + end],
            self.offering_run_threshold,
        );
        for &s in span {
            self.offering_run_slot[row + s.get()] =
                self.offering_run_slot[row + s.get()].saturating_sub(1);
        }
        let after = Self::run_excess_u32(
            &self.offering_run_slot[row + start..row + end],
            self.offering_run_threshold,
        );
        self.offering_run_excess_total = (i64::from(self.offering_run_excess_total)
            + i64::from(after)
            - i64::from(before)) as u32;
    }

    pub fn offering_run_cost(&self, weight: f64) -> f64 {
        self.offering_run_excess_total as f64 * weight
    }

    pub fn offering_run_ruin_cost(&self, offering: OfferingIdx, day: u32, weight: f64) -> f64 {
        if weight == 0.0 || self.offering_run_slot.is_empty() {
            return 0.0;
        }
        let (start, end) = self.day_range(day);
        let row = offering.get() * self.n_slots;
        if Self::run_excess_u32(
            &self.offering_run_slot[row + start..row + end],
            self.offering_run_threshold,
        ) > 0
        {
            weight
        } else {
            0.0
        }
    }

    /// Runs minus one, floored at zero — `0` for an empty day (nothing to
    /// split) or a single run (including a lone Session); `1` once a second
    /// separated run of the same Offering appears that day.
    #[inline]
    fn split_excess(runs: u32) -> u32 {
        runs.saturating_sub(1)
    }

    /// The split-excess DELTA `MinimizeOfferingDaySplit` would experience —
    /// mirrors [`Self::offering_run_delta`], `run_count_u32` instead of
    /// `run_excess_u32`.
    pub fn offering_split_delta(&self, offering: OfferingIdx, day: u32, span: &[SlotIdx]) -> i64 {
        if self.offering_split_slot.is_empty() || span.is_empty() {
            return 0;
        }
        let (start, end) = self.day_range(day);
        let row = offering.get() * self.n_slots;
        let before = Self::split_excess(Self::run_count_u32(
            &self.offering_split_slot[row + start..row + end],
        ));
        let after = Self::split_excess(Self::run_count_u32_with(
            &self.offering_split_slot[row + start..row + end],
            start,
            span,
        ));
        i64::from(after) - i64::from(before)
    }

    pub fn add_offering_split(&mut self, offering: OfferingIdx, day: u32, span: &[SlotIdx]) {
        if self.offering_split_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        let row = offering.get() * self.n_slots;
        let before = Self::split_excess(Self::run_count_u32(
            &self.offering_split_slot[row + start..row + end],
        ));
        for &s in span {
            self.offering_split_slot[row + s.get()] += 1;
        }
        let after = Self::split_excess(Self::run_count_u32(
            &self.offering_split_slot[row + start..row + end],
        ));
        self.offering_split_excess_total = (i64::from(self.offering_split_excess_total)
            + i64::from(after)
            - i64::from(before)) as u32;
    }

    pub fn remove_offering_split(&mut self, offering: OfferingIdx, day: u32, span: &[SlotIdx]) {
        if self.offering_split_slot.is_empty() || span.is_empty() {
            return;
        }
        let (start, end) = self.day_range(day);
        let row = offering.get() * self.n_slots;
        let before = Self::split_excess(Self::run_count_u32(
            &self.offering_split_slot[row + start..row + end],
        ));
        for &s in span {
            self.offering_split_slot[row + s.get()] =
                self.offering_split_slot[row + s.get()].saturating_sub(1);
        }
        let after = Self::split_excess(Self::run_count_u32(
            &self.offering_split_slot[row + start..row + end],
        ));
        self.offering_split_excess_total = (i64::from(self.offering_split_excess_total)
            + i64::from(after)
            - i64::from(before)) as u32;
    }

    pub fn offering_split_cost(&self, weight: f64) -> f64 {
        self.offering_split_excess_total as f64 * weight
    }

    pub fn offering_split_ruin_cost(&self, offering: OfferingIdx, day: u32, weight: f64) -> f64 {
        if weight == 0.0 || self.offering_split_slot.is_empty() {
            return 0.0;
        }
        let (start, end) = self.day_range(day);
        let row = offering.get() * self.n_slots;
        if Self::split_excess(Self::run_count_u32(
            &self.offering_split_slot[row + start..row + end],
        )) > 0
        {
            weight
        } else {
            0.0
        }
    }

    // -- max weekly teaching load -----------------------------------------------

    pub fn teaching_load_rules(&self) -> &[MaxWeeklyTeachingLoadInstance] {
        &self.teaching_load_rules
    }

    #[inline]
    fn teaching_load_cell(&self, lecturer: PersonIdx, week: u32) -> usize {
        lecturer.get() * self.n_weeks + week as usize
    }

    #[inline]
    fn teaching_load_excess(count: u32, threshold: u32) -> u32 {
        count.saturating_sub(threshold)
    }

    /// How much of a Session's duration counts toward the cap — blocks if
    /// the winning instance asked for it, one Session otherwise.
    #[inline]
    fn teaching_load_amount(&self, duration_blocks: u32) -> u32 {
        if self.teaching_load_count_blocks { duration_blocks } else { 1 }
    }

    /// The teaching-load cost DELTA if `lecturers` gained a Session of
    /// `duration_blocks` in `week` — the read-only preview, mirroring
    /// [`Self::group_run_delta`]. Unlike the day-granularity types above,
    /// there is nothing to iterate per block: the count itself is the whole
    /// state.
    pub fn teaching_load_delta(
        &self,
        lecturers: impl IntoIterator<Item = PersonIdx>,
        week: u32,
        duration_blocks: u32,
    ) -> i64 {
        if self.teaching_load_week.is_empty() {
            return 0;
        }
        let amount = self.teaching_load_amount(duration_blocks);
        let mut delta = 0i64;
        for l in lecturers {
            let c = self.teaching_load_cell(l, week);
            let before = Self::teaching_load_excess(
                self.teaching_load_week[c],
                self.teaching_load_threshold,
            );
            let after = Self::teaching_load_excess(
                self.teaching_load_week[c] + amount,
                self.teaching_load_threshold,
            );
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    pub fn add_teaching_load(
        &mut self,
        lecturers: impl IntoIterator<Item = PersonIdx>,
        week: u32,
        duration_blocks: u32,
    ) {
        if self.teaching_load_week.is_empty() {
            return;
        }
        let amount = self.teaching_load_amount(duration_blocks);
        for l in lecturers {
            let c = self.teaching_load_cell(l, week);
            let before = Self::teaching_load_excess(
                self.teaching_load_week[c],
                self.teaching_load_threshold,
            );
            self.teaching_load_week[c] += amount;
            let after = Self::teaching_load_excess(
                self.teaching_load_week[c],
                self.teaching_load_threshold,
            );
            self.teaching_load_excess_total = (i64::from(self.teaching_load_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    pub fn remove_teaching_load(
        &mut self,
        lecturers: impl IntoIterator<Item = PersonIdx>,
        week: u32,
        duration_blocks: u32,
    ) {
        if self.teaching_load_week.is_empty() {
            return;
        }
        let amount = self.teaching_load_amount(duration_blocks);
        for l in lecturers {
            let c = self.teaching_load_cell(l, week);
            let before = Self::teaching_load_excess(
                self.teaching_load_week[c],
                self.teaching_load_threshold,
            );
            self.teaching_load_week[c] = self.teaching_load_week[c].saturating_sub(amount);
            let after = Self::teaching_load_excess(
                self.teaching_load_week[c],
                self.teaching_load_threshold,
            );
            self.teaching_load_excess_total = (i64::from(self.teaching_load_excess_total)
                + i64::from(after)
                - i64::from(before)) as u32;
        }
    }

    /// What the currently over-cap weekly loads cost, at the configured
    /// weight — O(1), read straight off the running total.
    pub fn teaching_load_cost(&self, weight: f64) -> f64 {
        self.teaching_load_excess_total as f64 * weight
    }

    /// Sum of `weight` over every currently over-cap `(lecturer, week)` cell
    /// this occupant's lecturers sit in, for `ruin_worst`'s attribution.
    pub fn teaching_load_ruin_cost(
        &self,
        lecturers: impl IntoIterator<Item = PersonIdx>,
        week: u32,
        weight: f64,
    ) -> f64 {
        if weight == 0.0 || self.teaching_load_week.is_empty() {
            return 0.0;
        }
        let mut cost = 0.0;
        for l in lecturers {
            let c = self.teaching_load_cell(l, week);
            if self.teaching_load_week[c] > self.teaching_load_threshold {
                cost += weight;
            }
        }
        cost
    }

    // -- exam spacing (same day) ------------------------------------------------

    pub fn exam_same_day_rules(&self) -> &[ExamSpacingSameDayInstance] {
        &self.exam_same_day_rules
    }

    pub fn add_exam_same_day(&mut self, groups: &[GroupIdx], days: &[u32]) {
        if self.exam_same_day.is_empty() {
            return;
        }
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                self.exam_same_day[c] += 1;
            }
        }
    }

    pub fn remove_exam_same_day(&mut self, groups: &[GroupIdx], days: &[u32]) {
        if self.exam_same_day.is_empty() {
            return;
        }
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                self.exam_same_day[c] = self.exam_same_day[c].saturating_sub(1);
            }
        }
    }

    /// Would adding an exam-kind Session for `groups` on `days` create or
    /// worsen a same-day clash? Mirrors [`Self::day_mix_allows`].
    pub fn exam_same_day_allows(&self, groups: &[GroupIdx], days: &[u32]) -> bool {
        for &g in groups {
            for &d in days {
                if self.exam_same_day[self.day_cell(g, d)] > 0 {
                    return false;
                }
            }
        }
        true
    }

    /// How many `(group, day)` cells currently hold 2+ exam-kind Sessions.
    /// Read fresh off the counters, like `day_mix_violations` — the number
    /// the objective charges for.
    pub fn exam_same_day_violations(&self) -> u32 {
        self.exam_same_day.iter().filter(|&&c| c >= 2).count() as u32
    }

    /// Total `(group, day)` cells — the bound `Problem::hard_penalty` needs,
    /// mirroring `day_mix_cell_count`.
    pub fn exam_same_day_cell_count(&self) -> usize {
        self.exam_same_day.len()
    }

    /// Sum of `weight` over every currently-clashing `(group, day)` cell this
    /// occupant sits in, for `ruin_worst`'s attribution. Mirrors
    /// `day_mix_violation_cost`.
    pub fn exam_same_day_violation_cost(
        &self,
        groups: &[GroupIdx],
        days: &[u32],
        weight: f64,
    ) -> f64 {
        if weight == 0.0 || self.exam_same_day.is_empty() {
            return 0.0;
        }
        let mut cost = 0.0;
        for &g in groups {
            for &d in days {
                if self.exam_same_day[self.day_cell(g, d)] >= 2 {
                    cost += weight;
                }
            }
        }
        cost
    }

    // -- exam spacing (window) --------------------------------------------------

    pub fn exam_window_rules(&self) -> &[ExamSpacingWindowInstance] {
        &self.exam_window_rules
    }

    /// Every exam-kind Session within `exam_window_threshold` days of `day`
    /// (inclusive both ends, `day` itself included) for one Group — a
    /// distance-`< threshold` pair exists exactly when this sum exceeds 1.
    /// Cheap: `threshold` is a handful of days, so the window is a small,
    /// fixed-size scan, not proportional to the whole term.
    #[inline]
    fn exam_window_sum(&self, group: GroupIdx, day: u32) -> u32 {
        let threshold = self.exam_window_threshold;
        if self.n_days == 0 {
            return 0;
        }
        let lo = day.saturating_sub(threshold.saturating_sub(1));
        let hi = (day + threshold.saturating_sub(1)).min(self.n_days as u32 - 1);
        let row = group.get() * self.n_days;
        (lo..=hi).map(|d| self.exam_window[row + d as usize]).sum()
    }

    pub fn add_exam_window(&mut self, groups: &[GroupIdx], days: &[u32]) {
        if self.exam_window.is_empty() {
            return;
        }
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                self.exam_window[c] += 1;
            }
        }
    }

    pub fn remove_exam_window(&mut self, groups: &[GroupIdx], days: &[u32]) {
        if self.exam_window.is_empty() {
            return;
        }
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                self.exam_window[c] = self.exam_window[c].saturating_sub(1);
            }
        }
    }

    /// Would adding an exam-kind Session for `groups` on `days` land within
    /// the window of an existing one? Mirrors [`Self::exam_same_day_allows`].
    pub fn exam_window_allows(&self, groups: &[GroupIdx], days: &[u32]) -> bool {
        for &g in groups {
            for &d in days {
                if self.exam_window_sum(g, d) > 0 {
                    return false;
                }
            }
        }
        true
    }

    /// How many `(group, day)` cells hold an exam-kind Session that has
    /// another one somewhere in its window. Read fresh, like
    /// `exam_same_day_violations`.
    pub fn exam_window_violations(&self) -> u32 {
        if self.exam_window.is_empty() {
            return 0;
        }
        (0..self.exam_window.len())
            .filter(|&c| {
                let g = GroupIdx((c / self.n_days) as u32);
                let d = (c % self.n_days) as u32;
                self.exam_window[c] > 0 && self.exam_window_sum(g, d) > 1
            })
            .count() as u32
    }

    /// Total `(group, day)` cells, mirroring `exam_same_day_cell_count`.
    pub fn exam_window_cell_count(&self) -> usize {
        self.exam_window.len()
    }

    /// Sum of `weight` over every currently-clustered `(group, day)` cell
    /// this occupant sits in, for `ruin_worst`'s attribution.
    pub fn exam_window_violation_cost(
        &self,
        groups: &[GroupIdx],
        days: &[u32],
        weight: f64,
    ) -> f64 {
        if weight == 0.0 || self.exam_window.is_empty() {
            return 0.0;
        }
        let mut cost = 0.0;
        for &g in groups {
            for &d in days {
                if self.exam_window_sum(g, d) > 1 {
                    cost += weight;
                }
            }
        }
        cost
    }

    // -- minimize weekday imbalance ---------------------------------------------

    pub fn imbalance_rules(&self) -> &[MinimizeWeekdayImbalanceInstance] {
        &self.imbalance_rules
    }

    #[inline]
    fn imbalance_week_range(&self, week: u32) -> (usize, usize) {
        let start = week as usize * self.active_days_count;
        (start, start + self.active_days_count)
    }

    /// Population variance of one Group's per-active-day Session counts for
    /// one week — 0.0 for a perfectly even week (every active day holds the
    /// same count), growing as the week clusters onto fewer days.
    #[inline]
    fn weekday_variance(week_cells: &[u32]) -> f64 {
        let k = week_cells.len();
        if k == 0 {
            return 0.0;
        }
        let sum: u32 = week_cells.iter().sum();
        let mean = f64::from(sum) / k as f64;
        week_cells
            .iter()
            .map(|&c| {
                let d = f64::from(c) - mean;
                d * d
            })
            .sum::<f64>()
            / k as f64
    }

    /// `weekday_variance`, but with one more occurrence at `position` —
    /// WITHOUT mutating or allocating, mirroring `gap_u32_with`. `position`
    /// is the weekday's index WITHIN the week (`day_index % active_days_count`).
    #[inline]
    fn weekday_variance_with(week_cells: &[u32], position: usize) -> f64 {
        let k = week_cells.len();
        if k == 0 {
            return 0.0;
        }
        let sum: u32 = week_cells.iter().sum::<u32>() + 1;
        let mean = f64::from(sum) / k as f64;
        week_cells
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let v = if i == position { c + 1 } else { c };
                let d = f64::from(v) - mean;
                d * d
            })
            .sum::<f64>()
            / k as f64
    }

    pub fn add_imbalance(&mut self, groups: &[GroupIdx], days: &[u32]) {
        if self.imbalance_day.is_empty() {
            return;
        }
        for &g in groups {
            for &d in days {
                self.imbalance_day[g.get() * self.n_days + d as usize] += 1;
            }
        }
    }

    pub fn remove_imbalance(&mut self, groups: &[GroupIdx], days: &[u32]) {
        if self.imbalance_day.is_empty() {
            return;
        }
        for &g in groups {
            for &d in days {
                let c = g.get() * self.n_days + d as usize;
                self.imbalance_day[c] = self.imbalance_day[c].saturating_sub(1);
            }
        }
    }

    /// The imbalance-variance DELTA `MinimizeWeekdayImbalance` would
    /// experience if `groups` gained one more Session on `days` — the
    /// read-only preview, mirroring `group_run_delta`. A ranking signal
    /// only: unlike the gap/run/span types, the objective reads
    /// `imbalance_cost` fresh off the counters rather than a maintained
    /// running total, since groups x weeks is smaller than the groups x days
    /// scale `day_mix_violations` already rescans safely.
    pub fn imbalance_delta(&self, groups: &[GroupIdx], days: &[u32]) -> f64 {
        if self.imbalance_day.is_empty() {
            return 0.0;
        }
        let mut delta = 0.0;
        for &d in days {
            let week = d / self.active_days_count as u32;
            let position = d as usize % self.active_days_count;
            let (start, end) = self.imbalance_week_range(week);
            for &g in groups {
                let row = g.get() * self.n_days;
                let cells = &self.imbalance_day[row + start..row + end];
                delta +=
                    Self::weekday_variance_with(cells, position) - Self::weekday_variance(cells);
            }
        }
        delta
    }

    /// What every Group's current weekday imbalance costs, at the configured
    /// weight — a full rescan over `groups x weeks`, like
    /// `day_mix_violations`'s rescan over `groups x days`; this is smaller.
    pub fn imbalance_cost(&self, weight: f64) -> f64 {
        if weight == 0.0 || self.imbalance_day.is_empty() {
            return 0.0;
        }
        let groups = self.imbalance_day.len() / self.n_days;
        let weeks = self.n_days / self.active_days_count;
        let mut total = 0.0;
        for g in 0..groups {
            let row = g * self.n_days;
            for w in 0..weeks {
                let start = row + w * self.active_days_count;
                total += Self::weekday_variance(
                    &self.imbalance_day[start..start + self.active_days_count],
                );
            }
        }
        total * weight
    }

    /// Sum of `weight` over every currently-imbalanced week this occupant's
    /// Groups sit in, for `ruin_worst`'s attribution — a flat charge per
    /// occupant of a nonzero-variance week, the same attribution convention
    /// `compactness_ruin_cost` uses.
    pub fn imbalance_ruin_cost(&self, groups: &[GroupIdx], days: &[u32], weight: f64) -> f64 {
        if weight == 0.0 || self.imbalance_day.is_empty() {
            return 0.0;
        }
        let mut cost = 0.0;
        for &d in days {
            let week = d / self.active_days_count as u32;
            let (start, end) = self.imbalance_week_range(week);
            for &g in groups {
                let row = g.get() * self.n_days;
                if Self::weekday_variance(&self.imbalance_day[row + start..row + end]) > 0.0 {
                    cost += weight;
                }
            }
        }
        cost
    }

    // -- max days / max consecutive days ----------------------------------------
    //
    // HARD, priced at `hard_penalty` rather than a construction filter — see
    // `crate::problem::Problem::hard_penalty` and ADR-0025. Both reduce the
    // SAME day-occupancy substrate (`day_cap_group`/`day_cap_person`), one
    // by DISTINCT-day count, the other by longest CONSECUTIVE run — read
    // fresh per (entity, week) cell, exactly like `imbalance_cost`, rather
    // than maintained as a running total: entities x weeks is the same
    // small scale `imbalance_cost`'s own doc already argues is safe to
    // rescan.

    pub fn max_days_rules(&self) -> &[MaxDaysInstance] {
        &self.max_days_rules
    }

    pub fn max_consecutive_days_rules(&self) -> &[MaxConsecutiveDaysInstance] {
        &self.max_consecutive_days_rules
    }

    pub fn add_group_day_cap(&mut self, groups: &[GroupIdx], day: u32) {
        if self.day_cap_group.is_empty() {
            return;
        }
        for &g in groups {
            self.day_cap_group[g.get() * self.n_days + day as usize] += 1;
        }
    }

    pub fn remove_group_day_cap(&mut self, groups: &[GroupIdx], day: u32) {
        if self.day_cap_group.is_empty() {
            return;
        }
        for &g in groups {
            let c = g.get() * self.n_days + day as usize;
            self.day_cap_group[c] = self.day_cap_group[c].saturating_sub(1);
        }
    }

    pub fn add_person_day_cap(&mut self, persons: &[PersonIdx], day: u32) {
        if self.day_cap_person.is_empty() {
            return;
        }
        for &p in persons {
            self.day_cap_person[p.get() * self.n_days + day as usize] += 1;
        }
    }

    pub fn remove_person_day_cap(&mut self, persons: &[PersonIdx], day: u32) {
        if self.day_cap_person.is_empty() {
            return;
        }
        for &p in persons {
            let c = p.get() * self.n_days + day as usize;
            self.day_cap_person[c] = self.day_cap_person[c].saturating_sub(1);
        }
    }

    /// Distinct nonzero cells in one week's slice — what `MaxDays` caps.
    #[inline]
    fn distinct_days_u32(week_cells: &[u32]) -> u32 {
        week_cells.iter().filter(|&&c| c > 0).count() as u32
    }

    /// Whether one week's slice violates `threshold`, under either
    /// reduction. `consecutive = true` selects the `MaxConsecutiveDays`
    /// reading (`run_excess_u32 > 0`, i.e. some run exceeds the cap);
    /// `false` selects the `MaxDays` reading (distinct-day count exceeds
    /// it).
    #[inline]
    fn day_cap_violated(week_cells: &[u32], threshold: u32, consecutive: bool) -> bool {
        if threshold == u32::MAX {
            return false;
        }
        if consecutive {
            Self::run_excess_u32(week_cells, threshold) > 0
        } else {
            Self::distinct_days_u32(week_cells) > threshold
        }
    }

    /// Total currently-violating `(entity, week)` cells across BOTH axes,
    /// for one reduction — the number that joins the objective's hard
    /// component, the same role `share_violations` plays for
    /// `MaxOnlineShare`.
    fn day_cap_violations(
        cells: &[u32],
        n_days: usize,
        active_days_count: usize,
        threshold: u32,
        consecutive: bool,
    ) -> u32 {
        if cells.is_empty() || threshold == u32::MAX {
            return 0;
        }
        let entities = cells.len() / n_days;
        let weeks = n_days / active_days_count;
        let mut violated = 0u32;
        for e in 0..entities {
            let row = e * n_days;
            for w in 0..weeks {
                let start = row + w * active_days_count;
                let week_cells = &cells[start..start + active_days_count];
                if Self::day_cap_violated(week_cells, threshold, consecutive) {
                    violated += 1;
                }
            }
        }
        violated
    }

    pub fn max_days_violations(&self) -> u32 {
        Self::day_cap_violations(
            &self.day_cap_group,
            self.n_days,
            self.active_days_count,
            self.max_days_group_threshold,
            false,
        ) + Self::day_cap_violations(
            &self.day_cap_person,
            self.n_days,
            self.active_days_count,
            self.max_days_person_threshold,
            false,
        )
    }

    pub fn max_consecutive_days_violations(&self) -> u32 {
        Self::day_cap_violations(
            &self.day_cap_group,
            self.n_days,
            self.active_days_count,
            self.max_consecutive_days_group_threshold,
            true,
        ) + Self::day_cap_violations(
            &self.day_cap_person,
            self.n_days,
            self.active_days_count,
            self.max_consecutive_days_person_threshold,
            true,
        )
    }

    /// The longest run of consecutive nonzero cells — what
    /// `MaxConsecutiveDays` reports as `observed`, distinct from
    /// `run_excess_u32`'s excess-over-threshold total (which sums every
    /// run's excess, not the single longest one).
    #[inline]
    fn longest_run_u32(cells: &[u32]) -> u32 {
        let mut longest = 0u32;
        let mut run = 0u32;
        for &c in cells {
            if c > 0 {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        longest
    }

    /// Every currently-violating `(entity, week)` cell for one axis, under
    /// `consecutive`'s reduction — for REPORTING, mirroring `violated_cells`'
    /// role for `MaxOnlineShare`. `observed` is the distinct-day count
    /// (`MaxDays`) or the longest run (`MaxConsecutiveDays`) that exceeded
    /// the threshold.
    fn day_cap_violated_cells(
        cells: &[u32],
        n_days: usize,
        active_days_count: usize,
        threshold: u32,
        consecutive: bool,
    ) -> Vec<(u32, u32, u32)> {
        let mut out = Vec::new();
        if cells.is_empty() || threshold == u32::MAX {
            return out;
        }
        let entities = cells.len() / n_days;
        let weeks = n_days / active_days_count;
        for e in 0..entities {
            let row = e * n_days;
            for w in 0..weeks {
                let start = row + w * active_days_count;
                let week_cells = &cells[start..start + active_days_count];
                if Self::day_cap_violated(week_cells, threshold, consecutive) {
                    let observed = if consecutive {
                        Self::longest_run_u32(week_cells)
                    } else {
                        Self::distinct_days_u32(week_cells)
                    };
                    out.push((e as u32, w as u32, observed));
                }
            }
        }
        out
    }

    /// `(is_person, entity_index, week, observed_distinct_days)` for every
    /// currently-violating `MaxDays` cell, both axes combined.
    pub fn max_days_violated_cells(&self) -> Vec<(bool, u32, u32, u32)> {
        Self::day_cap_violated_cells(
            &self.day_cap_group,
            self.n_days,
            self.active_days_count,
            self.max_days_group_threshold,
            false,
        )
        .into_iter()
        .map(|(e, w, o)| (false, e, w, o))
        .chain(
            Self::day_cap_violated_cells(
                &self.day_cap_person,
                self.n_days,
                self.active_days_count,
                self.max_days_person_threshold,
                false,
            )
            .into_iter()
            .map(|(e, w, o)| (true, e, w, o)),
        )
        .collect()
    }

    /// The `MaxConsecutiveDays` counterpart of `max_days_violated_cells`.
    pub fn max_consecutive_days_violated_cells(&self) -> Vec<(bool, u32, u32, u32)> {
        Self::day_cap_violated_cells(
            &self.day_cap_group,
            self.n_days,
            self.active_days_count,
            self.max_consecutive_days_group_threshold,
            true,
        )
        .into_iter()
        .map(|(e, w, o)| (false, e, w, o))
        .chain(
            Self::day_cap_violated_cells(
                &self.day_cap_person,
                self.n_days,
                self.active_days_count,
                self.max_consecutive_days_person_threshold,
                true,
            )
            .into_iter()
            .map(|(e, w, o)| (true, e, w, o)),
        )
        .collect()
    }

    /// Would `day` newly push this entity's week over `threshold`, under
    /// `consecutive`'s reduction? A ranking signal only — `false` whenever
    /// `day` is already occupied (no change) or the week is already
    /// violated (this candidate does not create the violation).
    fn day_cap_would_worsen(
        cells: &[u32],
        entity_row: usize,
        n_days: usize,
        active_days_count: usize,
        day: u32,
        threshold: u32,
        consecutive: bool,
    ) -> bool {
        if cells.is_empty() || threshold == u32::MAX {
            return false;
        }
        let position = day as usize % active_days_count;
        let week = day as usize / active_days_count;
        let start = entity_row * n_days + week * active_days_count;
        let week_cells = &cells[start..start + active_days_count];
        if week_cells[position] > 0 {
            return false;
        }
        if Self::day_cap_violated(week_cells, threshold, consecutive) {
            return false;
        }
        let mut tmp: Vec<u32> = week_cells.to_vec();
        tmp[position] = 1;
        Self::day_cap_violated(&tmp, threshold, consecutive)
    }

    pub fn group_max_days_would_worsen(&self, groups: &[GroupIdx], day: u32) -> bool {
        groups.iter().any(|g| {
            Self::day_cap_would_worsen(
                &self.day_cap_group,
                g.get(),
                self.n_days,
                self.active_days_count,
                day,
                self.max_days_group_threshold,
                false,
            )
        })
    }

    pub fn person_max_days_would_worsen(&self, persons: &[PersonIdx], day: u32) -> bool {
        persons.iter().any(|p| {
            Self::day_cap_would_worsen(
                &self.day_cap_person,
                p.get(),
                self.n_days,
                self.active_days_count,
                day,
                self.max_days_person_threshold,
                false,
            )
        })
    }

    pub fn group_max_consecutive_days_would_worsen(&self, groups: &[GroupIdx], day: u32) -> bool {
        groups.iter().any(|g| {
            Self::day_cap_would_worsen(
                &self.day_cap_group,
                g.get(),
                self.n_days,
                self.active_days_count,
                day,
                self.max_consecutive_days_group_threshold,
                true,
            )
        })
    }

    pub fn person_max_consecutive_days_would_worsen(
        &self,
        persons: &[PersonIdx],
        day: u32,
    ) -> bool {
        persons.iter().any(|p| {
            Self::day_cap_would_worsen(
                &self.day_cap_person,
                p.get(),
                self.n_days,
                self.active_days_count,
                day,
                self.max_consecutive_days_person_threshold,
                true,
            )
        })
    }

    // -- daybreak ----------------------------------------------------------
    //
    // SOFT, priced like `RoomTurnaroundBuffer` — a minimum-gap welfare
    // preference, not a structural rule, so HARD would risk making an
    // instance needlessly harder to solve. Occupancy only (no within-day
    // excess total): the cost belongs to a PAIR of consecutive teaching
    // days, not to either day's placements alone, so it is read fresh —
    // see `crate::solution::SearchState::daybreak_cost`, which combines
    // this occupancy with `Problem::grid_time` for the actual wall-clock
    // arithmetic (this module has no knowledge of `GridTime`).

    pub fn daybreak_rules(&self) -> &[DaybreakInstance] {
        &self.daybreak_rules
    }

    pub fn daybreak_group_threshold_minutes(&self) -> u32 {
        self.daybreak_group_threshold_minutes
    }

    pub fn daybreak_person_threshold_minutes(&self) -> u32 {
        self.daybreak_person_threshold_minutes
    }

    pub fn add_group_daybreak(&mut self, groups: &[GroupIdx], span: &[SlotIdx]) {
        if self.daybreak_group_slot.is_empty() || span.is_empty() {
            return;
        }
        for &g in groups {
            let row = g.get() * self.n_slots;
            for &s in span {
                self.daybreak_group_slot[row + s.get()] += 1;
            }
        }
    }

    pub fn remove_group_daybreak(&mut self, groups: &[GroupIdx], span: &[SlotIdx]) {
        if self.daybreak_group_slot.is_empty() || span.is_empty() {
            return;
        }
        for &g in groups {
            let row = g.get() * self.n_slots;
            for &s in span {
                let c = row + s.get();
                self.daybreak_group_slot[c] = self.daybreak_group_slot[c].saturating_sub(1);
            }
        }
    }

    pub fn add_person_daybreak(&mut self, persons: &[PersonIdx], span: &[SlotIdx]) {
        if self.daybreak_person_slot.is_empty() || span.is_empty() {
            return;
        }
        for &p in persons {
            let row = p.get() * self.n_slots;
            for &s in span {
                let c = row + s.get();
                self.daybreak_person_slot[c] = self.daybreak_person_slot[c].saturating_add(1);
            }
        }
    }

    pub fn remove_person_daybreak(&mut self, persons: &[PersonIdx], span: &[SlotIdx]) {
        if self.daybreak_person_slot.is_empty() || span.is_empty() {
            return;
        }
        for &p in persons {
            let row = p.get() * self.n_slots;
            for &s in span {
                let c = row + s.get();
                self.daybreak_person_slot[c] = self.daybreak_person_slot[c].saturating_sub(1);
            }
        }
    }

    #[inline]
    fn occupied_range_u32(day_cells: &[u32]) -> Option<(u32, u32)> {
        let mut first = None;
        let mut last = 0usize;
        for (i, &c) in day_cells.iter().enumerate() {
            if c > 0 {
                first.get_or_insert(i);
                last = i;
            }
        }
        first.map(|f| (f as u32, last as u32))
    }

    #[inline]
    fn occupied_range_u8(day_cells: &[u8]) -> Option<(u32, u32)> {
        let mut first = None;
        let mut last = 0usize;
        for (i, &c) in day_cells.iter().enumerate() {
            if c > 0 {
                first.get_or_insert(i);
                last = i;
            }
        }
        first.map(|f| (f as u32, last as u32))
    }

    /// The first and last occupied block index for `group` on `day`, or
    /// `None` if unoccupied — what `Daybreak` compares across a day
    /// boundary.
    pub fn group_occupied_range(&self, group: GroupIdx, day: u32) -> Option<(u32, u32)> {
        if self.daybreak_group_slot.is_empty() {
            return None;
        }
        let (start, end) = self.day_range(day);
        let row = group.get() * self.n_slots;
        Self::occupied_range_u32(&self.daybreak_group_slot[row + start..row + end])
    }

    /// See [`Self::group_occupied_range`]; the Person counterpart.
    pub fn person_occupied_range(&self, person: PersonIdx, day: u32) -> Option<(u32, u32)> {
        if self.daybreak_person_slot.is_empty() {
            return None;
        }
        let (start, end) = self.day_range(day);
        let row = person.get() * self.n_slots;
        Self::occupied_range_u8(&self.daybreak_person_slot[row + start..row + end])
    }

    // -- travel time between rooms -------------------------------------------
    //
    // SOFT, priced like `RoomTurnaroundBuffer`/`Daybreak`. Occupancy stores
    // the ROOM'S dense index rather than a count, since this type must
    // compare WHICH Room adjacent blocks used, not merely whether one was
    // occupied — the one array in this module keyed that way. `Aggregates`
    // has no knowledge of `Room.location`, so the site comparison and the
    // actual `GridTime` gap lookup live in
    // `crate::solution::SearchState::travel_cost`; this module only
    // answers "which Room, if any, did this entity occupy at this block".

    pub fn travel_rules(&self) -> &[TravelTimeInstance] {
        &self.travel_rules
    }

    pub fn travel_group_threshold_minutes(&self) -> u32 {
        self.travel_group_threshold_minutes
    }

    pub fn travel_person_threshold_minutes(&self) -> u32 {
        self.travel_person_threshold_minutes
    }

    pub fn add_group_travel(&mut self, groups: &[GroupIdx], room: RoomIdx, span: &[SlotIdx]) {
        if self.travel_group_slot.is_empty() || span.is_empty() {
            return;
        }
        for &g in groups {
            let row = g.get() * self.n_slots;
            for &s in span {
                self.travel_group_slot[row + s.get()] = room.get() as u32;
            }
        }
    }

    pub fn remove_group_travel(&mut self, groups: &[GroupIdx], span: &[SlotIdx]) {
        if self.travel_group_slot.is_empty() || span.is_empty() {
            return;
        }
        for &g in groups {
            let row = g.get() * self.n_slots;
            for &s in span {
                self.travel_group_slot[row + s.get()] = u32::MAX;
            }
        }
    }

    pub fn add_person_travel(&mut self, persons: &[PersonIdx], room: RoomIdx, span: &[SlotIdx]) {
        if self.travel_person_slot.is_empty() || span.is_empty() {
            return;
        }
        for &p in persons {
            let row = p.get() * self.n_slots;
            for &s in span {
                self.travel_person_slot[row + s.get()] = room.get() as u32;
            }
        }
    }

    pub fn remove_person_travel(&mut self, persons: &[PersonIdx], span: &[SlotIdx]) {
        if self.travel_person_slot.is_empty() || span.is_empty() {
            return;
        }
        for &p in persons {
            let row = p.get() * self.n_slots;
            for &s in span {
                self.travel_person_slot[row + s.get()] = u32::MAX;
            }
        }
    }

    /// The Room occupying `group`'s `block` on `day`, or `None` if
    /// unoccupied.
    pub fn group_room_at(&self, group: GroupIdx, day: u32, block: u32) -> Option<RoomIdx> {
        if self.travel_group_slot.is_empty() {
            return None;
        }
        let idx = group.get() * self.n_slots + day as usize * self.blocks_per_day + block as usize;
        let r = self.travel_group_slot[idx];
        (r != u32::MAX).then_some(RoomIdx(r))
    }

    /// See [`Self::group_room_at`]; the Person counterpart.
    pub fn person_room_at(&self, person: PersonIdx, day: u32, block: u32) -> Option<RoomIdx> {
        if self.travel_person_slot.is_empty() {
            return None;
        }
        let idx = person.get() * self.n_slots + day as usize * self.blocks_per_day + block as usize;
        let r = self.travel_person_slot[idx];
        (r != u32::MAX).then_some(RoomIdx(r))
    }

    // -- location change -------------------------------------------------------

    pub fn location_rules(&self) -> &[MinimizeLocationChangeInstance] {
        &self.location_rules
    }

    /// The distinct-location-excess DELTA `MinimizeLocationChange` would
    /// experience if `groups` gained one Session touching `locations` (already
    /// deduplicated by the caller) on `day` — the read-only preview, mirroring
    /// [`Self::group_span_delta`]. Unlike the run/span/gap counters, this does
    /// not need a before/after rescan of a whole row: `location_group_distinct`
    /// is already the maintained distinct count, so only locations this
    /// (group, day) does not already touch can move it.
    pub fn group_location_delta(&self, groups: &[GroupIdx], day: u32, locations: &[u32]) -> i64 {
        if self.location_group_loc.is_empty() || locations.is_empty() {
            return 0;
        }
        let mut delta = 0i64;
        for &g in groups {
            let cell = g.get() * self.n_days + day as usize;
            let row = cell * self.n_locations;
            let newly_touched = locations
                .iter()
                .filter(|&&loc| self.location_group_loc[row + loc as usize] == 0)
                .count() as u32;
            if newly_touched == 0 {
                continue;
            }
            let before =
                self.location_group_distinct[cell].saturating_sub(self.location_group_threshold);
            let after = (self.location_group_distinct[cell] + newly_touched)
                .saturating_sub(self.location_group_threshold);
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    /// See [`Self::group_location_delta`]; the Person counterpart, over
    /// `who.attendees` rather than `who.subtree_groups`.
    pub fn person_location_delta(&self, persons: &[PersonIdx], day: u32, locations: &[u32]) -> i64 {
        if self.location_person_loc.is_empty() || locations.is_empty() {
            return 0;
        }
        let mut delta = 0i64;
        for &p in persons {
            let cell = p.get() * self.n_days + day as usize;
            let row = cell * self.n_locations;
            let newly_touched = locations
                .iter()
                .filter(|&&loc| self.location_person_loc[row + loc as usize] == 0)
                .count() as u32;
            if newly_touched == 0 {
                continue;
            }
            let before =
                self.location_person_distinct[cell].saturating_sub(self.location_person_threshold);
            let after = (self.location_person_distinct[cell] + newly_touched)
                .saturating_sub(self.location_person_threshold);
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    /// Mark `locations` (deduplicated) touched by `groups` on `day`, updating
    /// `location_group_excess_total` by the exact delta — mirrors
    /// [`Self::add_group_span`], one location at a time rather than one
    /// row-rescan, since each location's contribution to the distinct count is
    /// independent and additive.
    pub fn add_group_location(&mut self, groups: &[GroupIdx], day: u32, locations: &[u32]) {
        if self.location_group_loc.is_empty() {
            return;
        }
        for &g in groups {
            let cell = g.get() * self.n_days + day as usize;
            let row = cell * self.n_locations;
            for &loc in locations {
                let idx = row + loc as usize;
                self.location_group_loc[idx] += 1;
                if self.location_group_loc[idx] == 1 {
                    let before = self.location_group_distinct[cell]
                        .saturating_sub(self.location_group_threshold);
                    self.location_group_distinct[cell] += 1;
                    let after = self.location_group_distinct[cell]
                        .saturating_sub(self.location_group_threshold);
                    self.location_group_excess_total += after - before;
                }
            }
        }
    }

    pub fn remove_group_location(&mut self, groups: &[GroupIdx], day: u32, locations: &[u32]) {
        if self.location_group_loc.is_empty() {
            return;
        }
        for &g in groups {
            let cell = g.get() * self.n_days + day as usize;
            let row = cell * self.n_locations;
            for &loc in locations {
                let idx = row + loc as usize;
                self.location_group_loc[idx] = self.location_group_loc[idx].saturating_sub(1);
                if self.location_group_loc[idx] == 0 {
                    let before = self.location_group_distinct[cell]
                        .saturating_sub(self.location_group_threshold);
                    self.location_group_distinct[cell] =
                        self.location_group_distinct[cell].saturating_sub(1);
                    let after = self.location_group_distinct[cell]
                        .saturating_sub(self.location_group_threshold);
                    self.location_group_excess_total -= before - after;
                }
            }
        }
    }

    /// See [`Self::add_group_location`]; the Person counterpart.
    pub fn add_person_location(&mut self, persons: &[PersonIdx], day: u32, locations: &[u32]) {
        if self.location_person_loc.is_empty() {
            return;
        }
        for &p in persons {
            let cell = p.get() * self.n_days + day as usize;
            let row = cell * self.n_locations;
            for &loc in locations {
                let idx = row + loc as usize;
                self.location_person_loc[idx] += 1;
                if self.location_person_loc[idx] == 1 {
                    let before = self.location_person_distinct[cell]
                        .saturating_sub(self.location_person_threshold);
                    self.location_person_distinct[cell] += 1;
                    let after = self.location_person_distinct[cell]
                        .saturating_sub(self.location_person_threshold);
                    self.location_person_excess_total += after - before;
                }
            }
        }
    }

    pub fn remove_person_location(&mut self, persons: &[PersonIdx], day: u32, locations: &[u32]) {
        if self.location_person_loc.is_empty() {
            return;
        }
        for &p in persons {
            let cell = p.get() * self.n_days + day as usize;
            let row = cell * self.n_locations;
            for &loc in locations {
                let idx = row + loc as usize;
                self.location_person_loc[idx] = self.location_person_loc[idx].saturating_sub(1);
                if self.location_person_loc[idx] == 0 {
                    let before = self.location_person_distinct[cell]
                        .saturating_sub(self.location_person_threshold);
                    self.location_person_distinct[cell] =
                        self.location_person_distinct[cell].saturating_sub(1);
                    let after = self.location_person_distinct[cell]
                        .saturating_sub(self.location_person_threshold);
                    self.location_person_excess_total -= before - after;
                }
            }
        }
    }

    /// What the currently over-cap distinct-location days cost, at the
    /// configured weight(s) — O(1), read straight off
    /// `location_group_excess_total`/`location_person_excess_total` rather
    /// than rescanned. Mirrors [`Self::max_daily_span_cost`].
    pub fn location_change_cost(&self, group_weight: f64, person_weight: f64) -> f64 {
        self.location_group_excess_total as f64 * group_weight
            + self.location_person_excess_total as f64 * person_weight
    }

    /// Sum of `weight` over every currently over-cap `(entity, day)` cell this
    /// occupant participates in, for `ruin_worst`'s attribution. Mirrors
    /// [`Self::max_daily_span_ruin_cost`].
    pub fn location_change_ruin_cost(
        &self,
        groups: &[GroupIdx],
        persons: &[PersonIdx],
        day: u32,
        group_weight: f64,
        person_weight: f64,
    ) -> f64 {
        let mut cost = 0.0;
        if group_weight != 0.0 && !self.location_group_distinct.is_empty() {
            for &g in groups {
                let cell = g.get() * self.n_days + day as usize;
                if self.location_group_distinct[cell] > self.location_group_threshold {
                    cost += group_weight;
                }
            }
        }
        if person_weight != 0.0 && !self.location_person_distinct.is_empty() {
            for &p in persons {
                let cell = p.get() * self.n_days + day as usize;
                if self.location_person_distinct[cell] > self.location_person_threshold {
                    cost += person_weight;
                }
            }
        }
        cost
    }

    // -- room turnaround buffer ------------------------------------------------

    pub fn turnaround_rules(&self) -> &[RoomTurnaroundBufferInstance] {
        &self.turnaround_rules
    }

    /// Whether any OTHER booking of `room` sits within `turnaround_buffer_blocks`
    /// immediately before `span`'s start or immediately after its end, on the
    /// same day — `(before, after)`. Always excludes `span` itself: a caller
    /// probes this BEFORE marking its own bits (`add_room_turnaround`) or
    /// AFTER clearing them (`remove_room_turnaround`), so the array never
    /// contains `span`'s own occupancy when this runs.
    ///
    /// THIS MUST USE `span`'s OWN KNOWN BOUNDARIES, not a rescan of the row:
    /// a plain per-slot occupancy count cannot tell two back-to-back Sessions
    /// apart from one long one — both look like one uninterrupted run — so
    /// there is no boundary to recover from the array alone. Anchoring on the
    /// CANDIDATE's own start/end is what makes a zero-gap adjacency (the most
    /// important case) detectable at all.
    fn turnaround_boundary_violations(
        &self,
        room: RoomIdx,
        day: u32,
        span: &[SlotIdx],
    ) -> (bool, bool) {
        if self.turnaround_room_slot.is_empty() || self.turnaround_buffer_blocks == 0 {
            return (false, false);
        }
        let row = room.get() * self.n_slots;
        let (day_start, day_end) = self.day_range(day);
        let buffer = self.turnaround_buffer_blocks as usize;
        let start = span[0].get();
        let end = start + span.len();

        let before_from = start.saturating_sub(buffer).max(day_start);
        let before =
            (before_from..start.min(day_end)).any(|c| self.turnaround_room_slot[row + c] > 0);

        let after_to = (end + buffer).min(day_end);
        let after = (end.max(day_start)..after_to).any(|c| self.turnaround_room_slot[row + c] > 0);

        (before, after)
    }

    /// The violation-count DELTA `RoomTurnaroundBuffer` would experience if
    /// `room` gained a booking at `span` — the read-only preview, mirroring
    /// [`Self::group_span_delta`]. A candidate can create AT MOST one
    /// violation with its immediate left neighbor and one with its immediate
    /// right neighbor.
    pub fn room_turnaround_delta(&self, room: RoomIdx, day: u32, span: &[SlotIdx]) -> i64 {
        if span.is_empty() {
            return 0;
        }
        let (before, after) = self.turnaround_boundary_violations(room, day, span);
        i64::from(before) + i64::from(after)
    }

    /// Mark `span` busy for `room`'s turnaround tracking, updating
    /// `turnaround_violations_total` by the exact delta. Probes neighbors
    /// BEFORE marking `span`'s own bits, so the probe never sees itself.
    pub fn add_room_turnaround(&mut self, room: RoomIdx, day: u32, span: &[SlotIdx]) {
        if self.turnaround_room_slot.is_empty() || span.is_empty() {
            return;
        }
        let (before, after) = self.turnaround_boundary_violations(room, day, span);
        self.turnaround_violations_total += u32::from(before) + u32::from(after);
        let row = room.get() * self.n_slots;
        for &s in span {
            self.turnaround_room_slot[row + s.get()] += 1;
        }
    }

    /// Clears `span`'s own bits FIRST, then probes — the exact mirror of
    /// [`Self::add_room_turnaround`], so the probe never sees itself either.
    pub fn remove_room_turnaround(&mut self, room: RoomIdx, day: u32, span: &[SlotIdx]) {
        if self.turnaround_room_slot.is_empty() || span.is_empty() {
            return;
        }
        let row = room.get() * self.n_slots;
        for &s in span {
            self.turnaround_room_slot[row + s.get()] =
                self.turnaround_room_slot[row + s.get()].saturating_sub(1);
        }
        let (before, after) = self.turnaround_boundary_violations(room, day, span);
        self.turnaround_violations_total -= u32::from(before) + u32::from(after);
    }

    /// What every currently-violating Room-adjacency boundary costs, at the
    /// configured weight — O(1), read straight off
    /// `turnaround_violations_total` rather than rescanned, for the reason
    /// that field's own doc gives.
    pub fn room_turnaround_cost(&self, weight: f64) -> f64 {
        self.turnaround_violations_total as f64 * weight
    }

    /// Sum of `weight` over every boundary `room`'s booking at `span`
    /// currently violates, for `ruin_worst`'s attribution — reuses the same
    /// probe `add`/`remove` do, since `span`'s own bits being marked or not
    /// does not affect a check that only reads OUTSIDE `span`.
    pub fn room_turnaround_ruin_cost(
        &self,
        room: RoomIdx,
        day: u32,
        span: &[SlotIdx],
        weight: f64,
    ) -> f64 {
        if weight == 0.0 || self.turnaround_room_slot.is_empty() || span.is_empty() {
            return 0.0;
        }
        let (before, after) = self.turnaround_boundary_violations(room, day, span);
        (u32::from(before) + u32::from(after)) as f64 * weight
    }

    // -- room churn --------------------------------------------------------

    pub fn churn_rules(&self) -> &[MinimizeRoomChurnInstance] {
        &self.churn_rules
    }

    /// The distinct-Room-excess DELTA `MinimizeRoomChurn` would experience if
    /// `groups` gained one Session touching `rooms` (already deduplicated by
    /// the caller) in `week` — mirrors [`Self::group_location_delta`]
    /// exactly, WEEK/ROOM in place of DAY/LOCATION.
    pub fn group_churn_delta(&self, groups: &[GroupIdx], week: u32, rooms: &[u32]) -> i64 {
        if self.churn_room.is_empty() || rooms.is_empty() {
            return 0;
        }
        let mut delta = 0i64;
        for &g in groups {
            let cell = g.get() * self.n_weeks + week as usize;
            let row = cell * self.n_rooms;
            let newly_touched = rooms
                .iter()
                .filter(|&&r| self.churn_room[row + r as usize] == 0)
                .count() as u32;
            if newly_touched == 0 {
                continue;
            }
            let before = self.churn_distinct[cell].saturating_sub(self.churn_threshold);
            let after =
                (self.churn_distinct[cell] + newly_touched).saturating_sub(self.churn_threshold);
            delta += i64::from(after) - i64::from(before);
        }
        delta
    }

    /// Mark `rooms` (deduplicated) touched by `groups` in `week`, updating
    /// `churn_excess_total` by the exact delta — mirrors
    /// [`Self::add_group_location`].
    pub fn add_group_churn(&mut self, groups: &[GroupIdx], week: u32, rooms: &[u32]) {
        if self.churn_room.is_empty() {
            return;
        }
        for &g in groups {
            let cell = g.get() * self.n_weeks + week as usize;
            let row = cell * self.n_rooms;
            for &r in rooms {
                let idx = row + r as usize;
                self.churn_room[idx] += 1;
                if self.churn_room[idx] == 1 {
                    let before = self.churn_distinct[cell].saturating_sub(self.churn_threshold);
                    self.churn_distinct[cell] += 1;
                    let after = self.churn_distinct[cell].saturating_sub(self.churn_threshold);
                    self.churn_excess_total += after - before;
                }
            }
        }
    }

    pub fn remove_group_churn(&mut self, groups: &[GroupIdx], week: u32, rooms: &[u32]) {
        if self.churn_room.is_empty() {
            return;
        }
        for &g in groups {
            let cell = g.get() * self.n_weeks + week as usize;
            let row = cell * self.n_rooms;
            for &r in rooms {
                let idx = row + r as usize;
                self.churn_room[idx] = self.churn_room[idx].saturating_sub(1);
                if self.churn_room[idx] == 0 {
                    let before = self.churn_distinct[cell].saturating_sub(self.churn_threshold);
                    self.churn_distinct[cell] = self.churn_distinct[cell].saturating_sub(1);
                    let after = self.churn_distinct[cell].saturating_sub(self.churn_threshold);
                    self.churn_excess_total -= before - after;
                }
            }
        }
    }

    /// What the currently over-cap distinct-Room weeks cost, at the
    /// configured weight — O(1), read straight off `churn_excess_total`
    /// rather than rescanned. Mirrors [`Self::location_change_cost`].
    pub fn churn_cost(&self, weight: f64) -> f64 {
        self.churn_excess_total as f64 * weight
    }

    /// Sum of `weight` over every currently over-cap `(Group, week)` cell
    /// this occupant participates in, for `ruin_worst`'s attribution.
    /// Mirrors [`Self::max_daily_span_ruin_cost`].
    pub fn churn_ruin_cost(&self, groups: &[GroupIdx], week: u32, weight: f64) -> f64 {
        if weight == 0.0 || self.churn_distinct.is_empty() {
            return 0.0;
        }
        let mut cost = 0.0;
        for &g in groups {
            let cell = g.get() * self.n_weeks + week as usize;
            if self.churn_distinct[cell] > self.churn_threshold {
                cost += weight;
            }
        }
        cost
    }

    // -- room consistency ----------------------------------------------------

    pub fn consistency_rules(&self) -> &[RoomConsistencyInstance] {
        &self.consistency_rules
    }

    /// Sessions NOT in the modal Room, for one Offering's row already
    /// narrowed to its `n_rooms` cells — `total - max`. `0` for an empty row
    /// (nothing placed yet) or one already unanimous.
    #[inline]
    fn consistency_excess(row_cells: &[u32]) -> u32 {
        let total: u32 = row_cells.iter().sum();
        let modal = row_cells.iter().copied().max().unwrap_or(0);
        total - modal
    }

    /// The excess DELTA `RoomConsistency` would experience if this Offering
    /// gained one more Session in `room` — the read-only preview, mirroring
    /// [`Self::group_span_delta`]. One row scan, no allocation: adding one
    /// occurrence always raises the total by 1, and raises the max only when
    /// `room`'s own count was already (tied for) the max.
    pub fn room_consistency_delta(&self, offering: OfferingIdx, room: RoomIdx) -> i64 {
        if self.consistency_room.is_empty() {
            return 0;
        }
        let row_start = offering.get() * self.n_rooms;
        let row = &self.consistency_room[row_start..row_start + self.n_rooms];
        let total: u32 = row.iter().sum();
        let max = row.iter().copied().max().unwrap_or(0);
        let before = total - max;
        let after_total = total + 1;
        let after_max = max.max(row[room.get()] + 1);
        let after = after_total - after_max;
        i64::from(after) - i64::from(before)
    }

    /// Mark `room` as this Offering's Session, updating
    /// `consistency_excess_total` by the exact delta. A full row rescan on
    /// EVERY call, not only some — see that field's own doc for why the
    /// modal Room cannot be tracked with a cheaper incremental rule the way
    /// the distinct-count types above are.
    pub fn add_room_consistency(&mut self, offering: OfferingIdx, room: RoomIdx) {
        if self.consistency_room.is_empty() {
            return;
        }
        let row_start = offering.get() * self.n_rooms;
        let row_end = row_start + self.n_rooms;
        let before = Self::consistency_excess(&self.consistency_room[row_start..row_end]);
        self.consistency_room[row_start + room.get()] += 1;
        let after = Self::consistency_excess(&self.consistency_room[row_start..row_end]);
        self.consistency_excess_total = (i64::from(self.consistency_excess_total)
            + i64::from(after)
            - i64::from(before)) as u32;
    }

    pub fn remove_room_consistency(&mut self, offering: OfferingIdx, room: RoomIdx) {
        if self.consistency_room.is_empty() {
            return;
        }
        let row_start = offering.get() * self.n_rooms;
        let row_end = row_start + self.n_rooms;
        let before = Self::consistency_excess(&self.consistency_room[row_start..row_end]);
        self.consistency_room[row_start + room.get()] =
            self.consistency_room[row_start + room.get()].saturating_sub(1);
        let after = Self::consistency_excess(&self.consistency_room[row_start..row_end]);
        self.consistency_excess_total = (i64::from(self.consistency_excess_total)
            + i64::from(after)
            - i64::from(before)) as u32;
    }

    /// What every currently-inconsistent Offering costs, at the configured
    /// weight — O(1), read straight off `consistency_excess_total` rather
    /// than rescanned.
    pub fn consistency_cost(&self, weight: f64) -> f64 {
        self.consistency_excess_total as f64 * weight
    }

    /// Whether this Offering's Sessions are currently unanimous on one Room —
    /// for `ruin_worst`'s attribution: a flat charge for any occupant of an
    /// Offering that currently has ANY excess, the same attribution
    /// convention `distributed_ruin_cost`/`block_ruin_cost` use.
    pub fn consistency_ruin_cost(&self, offering: OfferingIdx, weight: f64) -> f64 {
        if weight == 0.0 || self.consistency_room.is_empty() {
            return 0.0;
        }
        let row_start = offering.get() * self.n_rooms;
        let row_end = row_start + self.n_rooms;
        if Self::consistency_excess(&self.consistency_room[row_start..row_end]) > 0 {
            weight
        } else {
            0.0
        }
    }

    // -- lecturer consistency -------------------------------------------------

    pub fn lecturer_consistency_rules(&self) -> &[LecturerConsistencyInstance] {
        &self.lecturer_consistency_rules
    }

    /// Distinct lecturers currently used past `required` — `0` for a row with
    /// nothing placed yet, or one that has never used more than `required`
    /// distinct identities.
    #[inline]
    fn lecturer_excess(row: &[(PersonIdx, u32)], required: u32) -> u32 {
        let distinct = row.iter().filter(|&&(_, count)| count > 0).count() as u32;
        distinct.saturating_sub(required)
    }

    /// The excess DELTA `LecturerConsistency` would experience if this
    /// Offering's placement gained `lecturers` — a read-only preview, ranking
    /// signal only like [`Self::room_consistency_delta`], not an exact charge:
    /// unlike that method this can add SEVERAL lecturers at once (a Session
    /// may need more than one), so the preview has to reason about the whole
    /// candidate set together rather than one Room. No allocation, no
    /// mutation: a lecturer counts as newly distinct only if it is neither
    /// already used (with a nonzero count) nor repeated earlier in this same
    /// candidate slice.
    pub fn lecturer_consistency_delta(
        &self,
        offering: OfferingIdx,
        lecturers: &[PersonIdx],
        required: u32,
    ) -> i64 {
        if self.lecturer_rows.is_empty() || lecturers.is_empty() {
            return 0;
        }
        let row = &self.lecturer_rows[offering.get()];
        let before = Self::lecturer_excess(row, required);
        let distinct_before = row.iter().filter(|&&(_, count)| count > 0).count() as u32;
        let mut new_distinct = 0u32;
        for (i, &p) in lecturers.iter().enumerate() {
            let already_used = row.iter().any(|&(id, count)| id == p && count > 0);
            let seen_earlier_in_batch = lecturers[..i].contains(&p);
            if !already_used && !seen_earlier_in_batch {
                new_distinct += 1;
            }
        }
        let after = (distinct_before + new_distinct).saturating_sub(required);
        i64::from(after) - i64::from(before)
    }

    /// Mark `lecturers` as this Offering's Session, updating
    /// `lecturer_excess_total` by the exact delta. A no-op for an Offering
    /// with no row (not a genuine lecturer pool, or the type not configured).
    pub fn add_lecturer_consistency(
        &mut self,
        offering: OfferingIdx,
        lecturers: &[PersonIdx],
        required: u32,
    ) {
        if self.lecturer_rows.is_empty() || lecturers.is_empty() {
            return;
        }
        let row = &mut self.lecturer_rows[offering.get()];
        let before = Self::lecturer_excess(row, required);
        for &p in lecturers {
            if let Some(entry) = row.iter_mut().find(|(id, _)| *id == p) {
                entry.1 += 1;
            } else {
                row.push((p, 1));
            }
        }
        let after = Self::lecturer_excess(row, required);
        self.lecturer_excess_total =
            (i64::from(self.lecturer_excess_total) + i64::from(after) - i64::from(before)) as u32;
    }

    pub fn remove_lecturer_consistency(
        &mut self,
        offering: OfferingIdx,
        lecturers: &[PersonIdx],
        required: u32,
    ) {
        if self.lecturer_rows.is_empty() || lecturers.is_empty() {
            return;
        }
        let row = &mut self.lecturer_rows[offering.get()];
        let before = Self::lecturer_excess(row, required);
        for &p in lecturers {
            if let Some(entry) = row.iter_mut().find(|(id, _)| *id == p) {
                entry.1 = entry.1.saturating_sub(1);
            }
        }
        let after = Self::lecturer_excess(row, required);
        self.lecturer_excess_total =
            (i64::from(self.lecturer_excess_total) + i64::from(after) - i64::from(before)) as u32;
    }

    /// What every currently-inconsistent Offering costs, at the configured
    /// weight — O(1), read straight off `lecturer_excess_total`.
    pub fn lecturer_consistency_cost(&self, weight: f64) -> f64 {
        self.lecturer_excess_total as f64 * weight
    }

    /// Flat charge for any occupant of an Offering that currently has ANY
    /// lecturer excess — the same attribution convention
    /// `consistency_ruin_cost` uses.
    pub fn lecturer_consistency_ruin_cost(
        &self,
        offering: OfferingIdx,
        required: u32,
        weight: f64,
    ) -> f64 {
        if weight == 0.0 || self.lecturer_rows.is_empty() {
            return 0.0;
        }
        let row = &self.lecturer_rows[offering.get()];
        if Self::lecturer_excess(row, required) > 0 { weight } else { 0.0 }
    }

    // -- scheduling pattern ---------------------------------------------------

    pub fn distributed_rules(&self) -> &[PatternAdherenceInstance] {
        &self.distributed_rules
    }

    pub fn block_rules(&self) -> &[PatternAdherenceInstance] {
        &self.block_rules
    }

    #[inline]
    fn distributed_index(&self, offering: OfferingIdx, cell: usize) -> usize {
        offering.get() * self.weekly_cells + cell
    }

    /// Mark one Session of `offering` occupying weekly `cell`, updating
    /// `distributed_total` by the exact delta. A no-op when
    /// `DistributedPatternAdherence` is not configured for any kind.
    pub fn add_distributed(&mut self, offering: OfferingIdx, cell: usize) {
        if self.distributed_cell.is_empty() {
            return;
        }
        let idx = self.distributed_index(offering, cell);
        self.distributed_cell[idx] += 1;
        if self.distributed_cell[idx] == 1 {
            let o = offering.get();
            let before = self.distributed_nonzero[o].saturating_sub(1);
            self.distributed_nonzero[o] += 1;
            let after = self.distributed_nonzero[o].saturating_sub(1);
            self.distributed_total += after - before;
        }
    }

    pub fn remove_distributed(&mut self, offering: OfferingIdx, cell: usize) {
        if self.distributed_cell.is_empty() {
            return;
        }
        let idx = self.distributed_index(offering, cell);
        self.distributed_cell[idx] = self.distributed_cell[idx].saturating_sub(1);
        if self.distributed_cell[idx] == 0 {
            let o = offering.get();
            let before = self.distributed_nonzero[o].saturating_sub(1);
            self.distributed_nonzero[o] = self.distributed_nonzero[o].saturating_sub(1);
            let after = self.distributed_nonzero[o].saturating_sub(1);
            self.distributed_total -= before - after;
        }
    }

    /// What every Offering's distinct-weekly-slot count over one currently
    /// costs, at the configured weight. O(1), read off `distributed_total`.
    pub fn distributed_cost(&self, weight: f64) -> f64 {
        self.distributed_total as f64 * weight
    }

    /// Read-only preview of `add_distributed`'s delta, for ranking a candidate
    /// before it is committed — see `group_compactness_delta`'s exact same
    /// contract.
    pub fn distributed_delta(&self, offering: OfferingIdx, cell: usize) -> i64 {
        if self.distributed_cell.is_empty() {
            return 0;
        }
        let idx = self.distributed_index(offering, cell);
        if self.distributed_cell[idx] > 0 {
            return 0; // this weekly slot is already in use; one more Session there adds nothing new
        }
        let o = offering.get();
        let before = self.distributed_nonzero[o].saturating_sub(1);
        let after = (self.distributed_nonzero[o] + 1).saturating_sub(1);
        (after - before) as i64
    }

    /// For `ruin_worst`: every placement of an Offering currently using more
    /// than one distinct weekly slot is charged, matching the day-mix/
    /// compactness convention of attributing to every occupant of a
    /// currently-bad cell rather than only the provably-useful ones.
    pub fn distributed_ruin_cost(&self, offering: OfferingIdx, weight: f64) -> f64 {
        if weight == 0.0 || self.distributed_nonzero.is_empty() {
            return 0.0;
        }
        if self.distributed_nonzero[offering.get()] > 1 { weight } else { 0.0 }
    }

    #[inline]
    fn block_row(&self, offering: OfferingIdx) -> usize {
        offering.get() * self.n_weeks
    }

    /// Mark one Session of `offering` occupying `week`, updating
    /// `block_gap_total` by the exact delta — the `Compactness` gap shape, at
    /// week granularity and scoped by Offering. A no-op when
    /// `BlockPatternAdherence` is not configured for any kind.
    pub fn add_block(&mut self, offering: OfferingIdx, week: u32) {
        if self.block_week.is_empty() {
            return;
        }
        let row = self.block_row(offering);
        let cells = &mut self.block_week[row..row + self.n_weeks];
        let before = Self::gap_u32(cells);
        cells[week as usize] += 1;
        let after = Self::gap_u32(cells);
        self.block_gap_total =
            (i64::from(self.block_gap_total) + i64::from(after) - i64::from(before)) as u32;
    }

    pub fn remove_block(&mut self, offering: OfferingIdx, week: u32) {
        if self.block_week.is_empty() {
            return;
        }
        let row = self.block_row(offering);
        let cells = &mut self.block_week[row..row + self.n_weeks];
        let before = Self::gap_u32(cells);
        cells[week as usize] = cells[week as usize].saturating_sub(1);
        let after = Self::gap_u32(cells);
        self.block_gap_total =
            (i64::from(self.block_gap_total) + i64::from(after) - i64::from(before)) as u32;
    }

    /// What every Offering's idle-weeks count currently costs, at the
    /// configured weight. O(1), read off `block_gap_total`.
    pub fn block_cost(&self, weight: f64) -> f64 {
        self.block_gap_total as f64 * weight
    }

    /// Read-only preview of `add_block`'s delta — see
    /// `group_compactness_delta`'s exact same contract.
    pub fn block_delta(&self, offering: OfferingIdx, week: u32) -> i64 {
        if self.block_week.is_empty() {
            return 0;
        }
        let row = self.block_row(offering);
        let cells = &self.block_week[row..row + self.n_weeks];
        let before = Self::gap_u32(cells);
        let after = Self::gap_u32_with_index(cells, week as usize);
        i64::from(after) - i64::from(before)
    }

    /// For `ruin_worst`: every placement of an Offering currently spanning any
    /// idle week is charged. Same convention as `distributed_ruin_cost`.
    pub fn block_ruin_cost(&self, offering: OfferingIdx, weight: f64) -> f64 {
        if weight == 0.0 || self.block_week.is_empty() {
            return 0.0;
        }
        let row = self.block_row(offering);
        if Self::gap_u32(&self.block_week[row..row + self.n_weeks]) > 0 {
            weight
        } else {
            0.0
        }
    }

    /// Like [`Self::gap_u32_with`], but treating a single index (a WEEK, not a
    /// span of slots) as already occupied. `BlockPatternAdherence`'s candidate
    /// touches exactly one week per Session, unlike a compactness Session's
    /// span which can cover more than one block.
    #[inline]
    fn gap_u32_with_index(cells: &[u32], extra: usize) -> u32 {
        let mut first = None;
        let mut last = 0usize;
        let mut occupied = 0u32;
        for (i, &c) in cells.iter().enumerate() {
            if c > 0 || i == extra {
                first.get_or_insert(i);
                last = i;
                occupied += 1;
            }
        }
        match first {
            Some(f) => (last - f + 1) as u32 - occupied,
            None => 0,
        }
    }

    // -- share -------------------------------------------------------------

    /// Add or remove one Session from every rule covering `kind`, keeping the
    /// running violation count exact by re-testing only the touched cells.
    pub fn apply_share(
        &mut self,
        kind: &str,
        groups: &[GroupIdx],
        week: u32,
        is_online: bool,
        add: bool,
    ) {
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.covers(kind) {
                continue;
            }
            let window = match rule.window {
                ShareWindow::PerTerm => 0,
                ShareWindow::PerWeek => week as usize,
            };
            let counters = &mut self.counters[i];
            if window >= counters.windows {
                continue;
            }

            for &g in groups {
                let cell = counters.cell(g, window);
                let before = counters.is_violated(rule, cell);

                if add {
                    counters.total[cell] += 1;
                    if is_online {
                        counters.online[cell] += 1;
                    }
                } else {
                    counters.total[cell] = counters.total[cell].saturating_sub(1);
                    if is_online {
                        counters.online[cell] = counters.online[cell].saturating_sub(1);
                    }
                }

                let after = counters.is_violated(rule, cell);
                match (before, after) {
                    (false, true) => counters.violated += 1,
                    (true, false) => counters.violated = counters.violated.saturating_sub(1),
                    _ => {}
                }
            }
        }
    }

    /// Sum of `hard_penalty` over every currently-violated `(rule, group,
    /// window)` cell this **online** Session participates in.
    ///
    /// For `ruin_worst`'s attribution convention (ADR-0025): a share breach
    /// belongs to a group's ratio, not to any one placement, so every online
    /// placement inside a breaching cell counts as responsible — removing one
    /// is the only way to bring `online` back under the cell's allowance.
    /// An on-site placement in the same cell is never charged: removing it
    /// only shrinks `total`, which can widen the violation rather than close
    /// it.
    pub fn share_violation_cost(
        &self,
        kind: &str,
        groups: &[GroupIdx],
        week: u32,
        hard_penalty: f64,
    ) -> f64 {
        let mut cost = 0.0;
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.covers(kind) {
                continue;
            }
            let window = match rule.window {
                ShareWindow::PerTerm => 0,
                ShareWindow::PerWeek => week as usize,
            };
            let counters = &self.counters[i];
            if window >= counters.windows {
                continue;
            }
            for &g in groups {
                let cell = counters.cell(g, window);
                if counters.is_violated(rule, cell) {
                    cost += hard_penalty;
                }
            }
        }
        cost
    }

    /// Sum of `weight` over every currently-mixed `(group, day)` cell this
    /// Session participates in, regardless of its own delivery mode.
    ///
    /// Unlike a share breach, either mode removed from a mixed cell can
    /// resolve it — the cell needs *a* removal, not specifically an online
    /// one — so both online and on-site placements in the cell are charged.
    pub fn day_mix_violation_cost(&self, groups: &[GroupIdx], days: &[u32], weight: f64) -> f64 {
        if weight == 0.0 {
            return 0.0;
        }
        let mut cost = 0.0;
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                if self.online_day[c] > 0 && self.onsite_day[c] > 0 {
                    cost += weight;
                }
            }
        }
        cost
    }

    /// Would adding one Session push any covering cell over its allowance?
    ///
    /// Used to *score* a candidate, never to reject it: `MaxOnlineShare` is an
    /// aggregate ratio, so a filter would dead-end construction before the
    /// denominator has grown.
    pub fn share_would_worsen(
        &self,
        kind: &str,
        groups: &[GroupIdx],
        week: u32,
        is_online: bool,
    ) -> bool {
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.covers(kind) {
                continue;
            }
            let window = match rule.window {
                ShareWindow::PerTerm => 0,
                ShareWindow::PerWeek => week as usize,
            };
            let counters = &self.counters[i];
            if window >= counters.windows {
                continue;
            }
            for &g in groups {
                let cell = counters.cell(g, window);
                let total = counters.total[cell] + 1;
                let online = counters.online[cell] + u32::from(is_online);
                if online > rule.allowance(total) {
                    return true;
                }
            }
        }
        false
    }

    /// Violated cells as `(rule index, group, window, online, total)`, for
    /// diagnostics and for the authoritative evaluator.
    pub fn violated_cells(&self) -> Vec<(usize, GroupIdx, usize, u32, u32)> {
        let mut out = Vec::new();
        for (i, counters) in self.counters.iter().enumerate() {
            let rule = &self.rules[i];
            for cell in 0..counters.total.len() {
                if counters.is_violated(rule, cell) {
                    out.push((
                        i,
                        GroupIdx((cell / counters.windows) as u32),
                        cell % counters.windows,
                        counters.online[cell],
                        counters.total[cell],
                    ));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(max_ratio: f64, window: ShareWindow) -> ShareInstance {
        ShareInstance { id: "s".into(), kinds: vec![], max_ratio, window }
    }

    #[test]
    fn allowance_floors_rather_than_rounds() {
        let r = rule(0.3, ShareWindow::PerTerm);
        assert_eq!(r.allowance(3), 0, "0.3 of 3 permits zero, not one");
        assert_eq!(r.allowance(4), 1);
        assert_eq!(r.allowance(10), 3);
        assert_eq!(rule(0.0, ShareWindow::PerTerm).allowance(100), 0);
        assert_eq!(rule(1.0, ShareWindow::PerTerm).allowance(7), 7);
    }

    #[test]
    fn day_mix_blocks_only_the_opposite_mode() {
        let mut a = Aggregates::new(
            2,
            3,
            1,
            vec![],
            0,
            0,
            1,
            vec![],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let g = [GroupIdx(0)];

        assert!(a.day_mix_allows(&g, &[0], true), "empty day accepts anything");
        a.add_day_mode(&g, &[0], true);

        assert!(a.day_mix_allows(&g, &[0], true), "another online session is fine");
        assert!(!a.day_mix_allows(&g, &[0], false), "on-site would make it a mix");

        // A different day, and a different group, are unaffected.
        assert!(a.day_mix_allows(&g, &[1], false));
        assert!(a.day_mix_allows(&[GroupIdx(1)], &[0], false));

        a.remove_day_mode(&g, &[0], true);
        assert!(a.day_mix_allows(&g, &[0], false), "removal reopens the day");
    }

    #[test]
    fn share_violation_tracks_a_moving_denominator() {
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![rule(0.5, ShareWindow::PerTerm)],
            0,
            0,
            1,
            vec![],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let g = [GroupIdx(0)];

        // 1 online of 1 total = 100% > 50%.
        a.apply_share("lecture", &g, 0, true, true);
        assert_eq!(a.share_violations(), 1);

        // Adding an on-site Session grows the denominator and clears it.
        a.apply_share("lecture", &g, 0, false, true);
        assert_eq!(a.share_violations(), 0, "1 of 2 is exactly 50%");

        // Removing the on-site one puts it back over.
        a.apply_share("lecture", &g, 0, false, false);
        assert_eq!(a.share_violations(), 1);
    }

    #[test]
    fn per_week_and_per_term_bucket_differently() {
        let g = [GroupIdx(0)];
        // Two online in week 0, two on-site in week 1, ratio 0.5.
        let load = |a: &mut Aggregates| {
            a.apply_share("lecture", &g, 0, true, true);
            a.apply_share("lecture", &g, 0, true, true);
            a.apply_share("lecture", &g, 1, false, true);
            a.apply_share("lecture", &g, 1, false, true);
        };

        // PER_TERM: 2 online of 4 = 50%, allowed.
        let mut term = Aggregates::new(
            1,
            1,
            2,
            vec![rule(0.5, ShareWindow::PerTerm)],
            0,
            0,
            1,
            vec![],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        load(&mut term);
        assert_eq!(term.share_violations(), 0);

        // PER_WEEK: week 0 is 2 of 2 = 100%, violated.
        let mut week = Aggregates::new(
            1,
            1,
            2,
            vec![rule(0.5, ShareWindow::PerWeek)],
            0,
            0,
            1,
            vec![],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        load(&mut week);
        assert_eq!(week.share_violations(), 1);
    }

    #[test]
    fn kind_scoping_skips_rules_that_do_not_cover() {
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![ShareInstance {
                id: "s".into(),
                kinds: vec!["lecture".into()],
                max_ratio: 0.0,
                window: ShareWindow::PerTerm,
            }],
            0,
            0,
            1,
            vec![],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        a.apply_share("staff_meeting", &[GroupIdx(0)], 0, true, true);
        assert_eq!(a.share_violations(), 0, "out-of-scope kind must not count");

        a.apply_share("lecture", &[GroupIdx(0)], 0, true, true);
        assert_eq!(a.share_violations(), 1);
    }

    /// ADR-0025: `ruin_worst` must be able to tell which placement removal
    /// could actually repair a breach. Only the **online** one can — removing
    /// the on-site Session only shrinks `total`, which cannot lower the
    /// online count back under the allowance.
    #[test]
    fn share_violation_cost_charges_the_online_session_not_the_onsite_one() {
        let scoped = ShareInstance {
            id: "s".into(),
            kinds: vec!["lecture".into()],
            max_ratio: 0.5,
            window: ShareWindow::PerTerm,
        };
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![scoped],
            0,
            0,
            1,
            vec![],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let g = [GroupIdx(0)];
        // 2 online of 2 total = 100% > 50%: violated.
        a.apply_share("lecture", &g, 0, true, true);
        a.apply_share("lecture", &g, 0, true, true);
        assert_eq!(a.share_violations(), 1);

        assert_eq!(
            a.share_violation_cost("lecture", &g, 0, 100.0),
            100.0,
            "one violated cell, charged at hard_penalty"
        );
        assert_eq!(
            a.day_mix_violation_cost(&g, &[0], 5.0),
            0.0,
            "no day-mix state was touched here"
        );

        // A rule that does not cover this kind must not charge either.
        assert_eq!(a.share_violation_cost("staff_meeting", &g, 0, 100.0), 0.0);
    }

    #[test]
    fn share_violation_cost_is_zero_once_the_cell_stops_violating() {
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![rule(0.5, ShareWindow::PerTerm)],
            0,
            0,
            1,
            vec![],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let g = [GroupIdx(0)];
        a.apply_share("lecture", &g, 0, true, true);
        assert_eq!(a.share_violation_cost("lecture", &g, 0, 100.0), 100.0);

        // Widening the denominator clears the violation (see
        // `share_violation_tracks_a_moving_denominator`), and the cost must
        // follow it back to zero rather than staying pinned.
        a.apply_share("lecture", &g, 0, false, true);
        assert_eq!(a.share_violation_cost("lecture", &g, 0, 100.0), 0.0);
    }

    /// Unlike a share breach, a mixed day can be resolved by removing
    /// *either* mode, so both the online and the on-site occupant of a mixed
    /// cell are charged — the asymmetry above does not apply here.
    #[test]
    fn day_mix_violation_cost_charges_both_modes_in_a_mixed_cell() {
        let mut a = Aggregates::new(
            1,
            2,
            1,
            vec![],
            0,
            0,
            1,
            vec![],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let g = [GroupIdx(0)];
        a.add_day_mode(&g, &[0], true);
        assert_eq!(a.day_mix_violation_cost(&g, &[0], 10.0), 0.0, "one mode alone is not mixed");

        a.add_day_mode(&g, &[0], false);
        assert_eq!(a.day_mix_violation_cost(&g, &[0], 10.0), 10.0, "now mixed");
        // A weight of zero (the rule disabled) must charge nothing, not skip
        // the check silently in some other way.
        assert_eq!(a.day_mix_violation_cost(&g, &[0], 0.0), 0.0);
        // An untouched day on the same group is unaffected.
        assert_eq!(a.day_mix_violation_cost(&g, &[1], 10.0), 0.0);
    }

    // -- compactness ---------------------------------------------------------

    fn compactness_rule(group: bool, person: bool) -> CompactnessInstance {
        CompactnessInstance { id: "c".into(), kinds: vec![], weight: 1.0, group, person }
    }

    #[test]
    fn group_compactness_counts_idle_blocks_between_first_and_last() {
        // 4 blocks/day, 1 day, 1 group.
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![],
            0,
            4,
            4,
            vec![compactness_rule(true, false)],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let g = [GroupIdx(0)];

        a.add_group_compactness(&g, 0, &[SlotIdx(0)]);
        assert_eq!(
            a.compactness_cost(1.0, 0.0),
            0.0,
            "a single occupied block has nothing between"
        );

        a.add_group_compactness(&g, 0, &[SlotIdx(3)]);
        assert_eq!(a.compactness_cost(1.0, 0.0), 2.0, "blocks 1 and 2 sit idle between 0 and 3");

        a.add_group_compactness(&g, 0, &[SlotIdx(1)]);
        assert_eq!(a.compactness_cost(1.0, 0.0), 1.0, "only block 2 is idle now");

        a.remove_group_compactness(&g, 0, &[SlotIdx(3)]);
        assert_eq!(a.compactness_cost(1.0, 0.0), 0.0, "0 and 1 are adjacent: no gap left");
    }

    #[test]
    fn compactness_cost_is_zero_at_zero_weight_regardless_of_gaps() {
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![],
            0,
            4,
            4,
            vec![compactness_rule(true, false)],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let g = [GroupIdx(0)];
        a.add_group_compactness(&g, 0, &[SlotIdx(0)]);
        a.add_group_compactness(&g, 0, &[SlotIdx(3)]);
        assert_eq!(a.compactness_cost(0.0, 0.0), 0.0, "a disabled weight must charge nothing");
    }

    #[test]
    fn person_compactness_uses_the_same_gap_rule_as_group() {
        let mut a = Aggregates::new(
            0,
            1,
            1,
            vec![],
            1,
            4,
            4,
            vec![compactness_rule(false, true)],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let p = [PersonIdx(0)];

        a.add_person_compactness(&p, 0, &[SlotIdx(0)]);
        a.add_person_compactness(&p, 0, &[SlotIdx(3)]);
        assert_eq!(a.compactness_cost(0.0, 1.0), 2.0);

        a.remove_person_compactness(&p, 0, &[SlotIdx(0)]);
        assert_eq!(a.compactness_cost(0.0, 1.0), 0.0, "one remaining block has nothing between");
    }

    #[test]
    fn an_axis_not_selected_by_any_rule_is_never_allocated_or_tracked() {
        // Only the Group axis is configured — Person tracking must be a no-op,
        // not silently inert data that still costs memory.
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![],
            1,
            4,
            4,
            vec![compactness_rule(true, false)],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        a.add_person_compactness(&[PersonIdx(0)], 0, &[SlotIdx(0), SlotIdx(3)]);
        assert_eq!(a.compactness_cost(0.0, 1.0), 0.0, "Person axis was never configured");
    }

    #[test]
    fn a_second_session_sharing_a_block_does_not_corrupt_removal() {
        // Two Sessions of the same Group legitimately occupying block 1 at
        // once (group_double_booking disabled) — removing ONE must not clear
        // the block the other still holds. A bitset would get this wrong; the
        // count must not.
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![],
            0,
            4,
            4,
            vec![compactness_rule(true, false)],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let g = [GroupIdx(0)];
        a.add_group_compactness(&g, 0, &[SlotIdx(0)]);
        a.add_group_compactness(&g, 0, &[SlotIdx(1)]);
        a.add_group_compactness(&g, 0, &[SlotIdx(1)]); // second Session, same block
        a.add_group_compactness(&g, 0, &[SlotIdx(3)]);
        assert_eq!(a.compactness_cost(1.0, 0.0), 1.0, "block 2 idle between 1 and 3");

        a.remove_group_compactness(&g, 0, &[SlotIdx(1)]); // remove only one of the two
        assert_eq!(
            a.compactness_cost(1.0, 0.0),
            1.0,
            "block 1 is still occupied by the other Session — still no gap there"
        );
    }

    #[test]
    fn the_read_only_delta_preview_matches_what_add_actually_charges() {
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![],
            0,
            4,
            4,
            vec![compactness_rule(true, false)],
            0,
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let g = [GroupIdx(0)];
        a.add_group_compactness(&g, 0, &[SlotIdx(0)]);

        let preview = a.group_compactness_delta(&g, 0, &[SlotIdx(3)]);
        let before = a.compactness_cost(1.0, 0.0);
        a.add_group_compactness(&g, 0, &[SlotIdx(3)]);
        let after = a.compactness_cost(1.0, 0.0);

        assert_eq!(
            preview as f64,
            after - before,
            "the preview must match the real charge exactly"
        );
    }

    // -- scheduling pattern ---------------------------------------------------

    fn pattern_rule() -> PatternAdherenceInstance {
        PatternAdherenceInstance { id: "p".into(), kinds: vec![], weight: 1.0 }
    }

    #[test]
    fn distributed_costs_nothing_for_one_consistent_weekly_slot() {
        // 3 weeks, weekly_cells = 2 (say Monday/Tuesday), 1 Offering.
        let mut a = Aggregates::new(
            0,
            0,
            3,
            vec![],
            0,
            0,
            1,
            vec![],
            1,
            2,
            vec![pattern_rule()],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let o = OfferingIdx(0);
        a.add_distributed(o, 0);
        a.add_distributed(o, 0);
        a.add_distributed(o, 0);
        assert_eq!(a.distributed_cost(1.0), 0.0, "every Session shares one weekly slot");
    }

    #[test]
    fn distributed_charges_per_extra_distinct_slot() {
        let mut a = Aggregates::new(
            0,
            0,
            3,
            vec![],
            0,
            0,
            1,
            vec![],
            1,
            2,
            vec![pattern_rule()],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let o = OfferingIdx(0);
        a.add_distributed(o, 0);
        assert_eq!(a.distributed_cost(1.0), 0.0, "the first slot used is free");
        a.add_distributed(o, 1);
        assert_eq!(a.distributed_cost(1.0), 1.0, "a second distinct slot costs one");

        a.remove_distributed(o, 1);
        assert_eq!(a.distributed_cost(1.0), 0.0, "back to one slot, back to zero");
    }

    #[test]
    fn distributed_delta_preview_matches_the_real_charge() {
        let mut a = Aggregates::new(
            0,
            0,
            3,
            vec![],
            0,
            0,
            1,
            vec![],
            1,
            2,
            vec![pattern_rule()],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let o = OfferingIdx(0);
        a.add_distributed(o, 0);

        let preview = a.distributed_delta(o, 1);
        let before = a.distributed_cost(1.0);
        a.add_distributed(o, 1);
        let after = a.distributed_cost(1.0);
        assert_eq!(preview as f64, after - before);
    }

    #[test]
    fn distributed_is_independent_per_offering() {
        let mut a = Aggregates::new(
            0,
            0,
            3,
            vec![],
            0,
            0,
            1,
            vec![],
            2,
            2,
            vec![pattern_rule()],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        a.add_distributed(OfferingIdx(0), 0);
        a.add_distributed(OfferingIdx(0), 1);
        // Offering 1 uses only one slot: must not be charged for Offering 0's spread.
        a.add_distributed(OfferingIdx(1), 0);
        assert_eq!(a.distributed_cost(1.0), 1.0, "only Offering 0 contributes");
    }

    #[test]
    fn block_counts_idle_weeks_between_first_and_last() {
        // 5 weeks, 1 Offering.
        let mut a = Aggregates::new(
            0,
            0,
            5,
            vec![],
            0,
            0,
            1,
            vec![],
            1,
            1,
            vec![],
            vec![pattern_rule()],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let o = OfferingIdx(0);
        a.add_block(o, 0);
        assert_eq!(a.block_cost(1.0), 0.0, "one week has nothing between");

        a.add_block(o, 3);
        assert_eq!(a.block_cost(1.0), 2.0, "weeks 1 and 2 sit idle between 0 and 3");

        a.add_block(o, 1);
        assert_eq!(a.block_cost(1.0), 1.0, "only week 2 is idle now");

        a.remove_block(o, 3);
        assert_eq!(a.block_cost(1.0), 0.0, "0 and 1 are adjacent: no gap left");
    }

    #[test]
    fn block_delta_preview_matches_the_real_charge() {
        let mut a = Aggregates::new(
            0,
            0,
            5,
            vec![],
            0,
            0,
            1,
            vec![],
            1,
            1,
            vec![],
            vec![pattern_rule()],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        let o = OfferingIdx(0);
        a.add_block(o, 0);

        let preview = a.block_delta(o, 3);
        let before = a.block_cost(1.0);
        a.add_block(o, 3);
        let after = a.block_cost(1.0);
        assert_eq!(preview as f64, after - before);
    }

    #[test]
    fn an_axis_not_configured_costs_nothing_regardless_of_activity() {
        // Only BLOCK is configured; DISTRIBUTED tracking must be entirely
        // absent, not merely zero-weighted.
        let mut a = Aggregates::new(
            0,
            0,
            5,
            vec![],
            0,
            0,
            1,
            vec![],
            1,
            2,
            vec![],
            vec![pattern_rule()],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            vec![],
            vec![],
            1,
            vec![],
            1,
            vec![],
            vec![],
            vec![], // lecturer_consistency_rules
            vec![], // daybreak_rules
            vec![], // travel_rules
        );
        a.add_distributed(OfferingIdx(0), 0);
        a.add_distributed(OfferingIdx(0), 1);
        assert_eq!(a.distributed_cost(1.0), 0.0, "distributed was never configured");
    }
}
