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
        teaching_load_rules: Vec<MaxWeeklyTeachingLoadInstance>,
        exam_same_day_rules: Vec<ExamSpacingSameDayInstance>,
        exam_window_rules: Vec<ExamSpacingWindowInstance>,
        imbalance_rules: Vec<MinimizeWeekdayImbalanceInstance>,
        active_days_count: usize,
        location_rules: Vec<MinimizeLocationChangeInstance>,
        n_locations: usize,
        turnaround_rules: Vec<RoomTurnaroundBufferInstance>,
        n_rooms: usize,
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
        lecturers: &[PersonIdx],
        week: u32,
        duration_blocks: u32,
    ) -> i64 {
        if self.teaching_load_week.is_empty() {
            return 0;
        }
        let amount = self.teaching_load_amount(duration_blocks);
        let mut delta = 0i64;
        for &l in lecturers {
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

    pub fn add_teaching_load(&mut self, lecturers: &[PersonIdx], week: u32, duration_blocks: u32) {
        if self.teaching_load_week.is_empty() {
            return;
        }
        let amount = self.teaching_load_amount(duration_blocks);
        for &l in lecturers {
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
        lecturers: &[PersonIdx],
        week: u32,
        duration_blocks: u32,
    ) {
        if self.teaching_load_week.is_empty() {
            return;
        }
        let amount = self.teaching_load_amount(duration_blocks);
        for &l in lecturers {
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
    pub fn teaching_load_ruin_cost(&self, lecturers: &[PersonIdx], week: u32, weight: f64) -> f64 {
        if weight == 0.0 || self.teaching_load_week.is_empty() {
            return 0.0;
        }
        let mut cost = 0.0;
        for &l in lecturers {
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
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
            1,
            vec![],
            1,
            vec![],
            1,
        );
        a.add_distributed(OfferingIdx(0), 0);
        a.add_distributed(OfferingIdx(0), 1);
        assert_eq!(a.distributed_cost(1.0), 0.0, "distributed was never configured");
    }
}
