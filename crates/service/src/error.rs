//! What can be wrong with a caller's snapshot, and the single place that maps it
//! onto gRPC.
//!
//! Every validation predicate in [`crate::convert`] used to construct its own
//! `tonic::Status` in place, at 21 sites. Three costs came with that:
//!
//! 1. **The transport type was the conversion module's error type**, so
//!    validation could not be exercised, snapshot-tested or reused without
//!    linking `tonic` — which is a large part of why none of the 21 paths had a
//!    test.
//! 2. **The code-selection policy was scattered across 21 sites** with no single
//!    place to change it, and it had already drifted: an unsupported lecturer
//!    pool (an *input* the caller can fix) returned `UNIMPLEMENTED` alongside
//!    genuinely unbuilt features.
//! 3. **Core's typed errors were flattened to prose.** `Problem::build` returns
//!    a `GroupCycle` naming the groups involved; that became
//!    `Status::invalid_argument(c.to_string())`, so a caller wanting to
//!    distinguish "your group hierarchy has a cycle" from "your time grid is
//!    malformed" had to match on message text.
//!
//! ADR-0004 anticipated this: conversion is deliberately not its own crate yet,
//! and is promoted when its validation logic grows. 21 validation sites in a
//! 700-line implementation is that growth. This module is the step that makes the
//! crate split mechanical — a `calendry-solver-convert` crate would depend on
//! `core` and `proto` but not on `tonic` — while deliberately stopping short of
//! taking it. See
//! `docs/adr/0017-conversion-errors-are-typed-transport-mapping-is-one-place.md`.

use calendry_solver_core::GroupCycle;
use tonic::Status;

/// A caller snapshot the solver cannot turn into a [`Problem`].
///
/// Variants name the **domain** fault, not the transport response. The mapping
/// to a gRPC code is [`From<ConvertError> for Status`], and lives in exactly one
/// place.
///
/// [`Problem`]: calendry_solver_core::Problem
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConvertError {
    // -- required fields -----------------------------------------------------
    #[error("input.time_grid is required")]
    MissingTimeGrid,
    #[error("input.calendar is required")]
    MissingCalendar,
    #[error("request.input is required")]
    MissingInput,
    #[error("request.scope is required")]
    MissingScope,

    // -- the grid ------------------------------------------------------------
    #[error("invalid time grid: {reason}")]
    InvalidTimeGrid { reason: String },

    // -- entity references ---------------------------------------------------
    #[error("group '{group}' names unknown parent '{parent}'")]
    UnknownGroupParent { group: String, parent: String },
    #[error("{context} references unknown room '{room}'")]
    UnknownRoom { context: String, room: String },
    #[error("{context} references unknown person '{person}'")]
    UnknownPerson { context: String, person: String },
    #[error("{context} references unknown group '{group}'")]
    UnknownGroup { context: String, group: String },
    #[error("{context} references unknown offering '{offering}'")]
    UnknownOffering { context: String, offering: String },
    #[error(transparent)]
    GroupCycle(#[from] GroupCycle),

    // -- rooms ---------------------------------------------------------------
    #[error(
        "room '{room}' is virtual and carries footprint_tags {tags:?}; a footprint is a claim \
         about physical space, and a virtual room has none — its occupancy is deliberately \
         unlimited, so the tag could only ever be inert. An inert exclusivity is the worst \
         outcome available: the run would report no violation while the space it names is \
         double-booked every time"
    )]
    FootprintOnVirtualRoom { room: String, tags: Vec<String> },

    // -- calendar ------------------------------------------------------------
    /// `Week.exam_group_ids` on a week that is not an exam week.
    ///
    /// Refused rather than ignored for `FootprintOnVirtualRoom`'s reason: the
    /// scope narrows an exam period this week does not have, so it could only
    /// ever be inert — and inert here reads as "no exam period at all", which
    /// puts ordinary teaching on top of the exams the scope was sent to
    /// protect while the run reports nothing wrong.
    #[error(
        "calendar.weeks[{week}] carries exam_group_ids {groups:?} but its kind is not \
         WEEK_KIND_EXAM; the scope narrows an exam period this week does not have, so it could \
         only ever be inert — and an inert scope reads as 'no exam period', putting ordinary \
         teaching on top of the exams it was sent to protect while the run reports nothing wrong"
    )]
    ExamGroupsOnNonExamWeek { week: u32, groups: Vec<String> },

    // -- offerings -----------------------------------------------------------
    /// `required_lecturer_count: 0` on an Offering that still lists candidate
    /// lecturers.
    ///
    /// Refused because the alternative is not inert, it is WRONG: the pool
    /// branch needs `required >= 1`, so a zero count falls through to the
    /// fixed-assignment path and every listed candidate is assigned to every
    /// Session — the opposite of the "the solver picks one" rule the count is
    /// derived to express. Calendry #130.
    #[error(
        "offering '{offering}' requires 0 lecturers but lists {candidates} candidate(s); that \
         does not mean 'nobody teaches this', it means every one of those candidates is \
         assigned to every Session, which is the opposite of what a candidate POOL is for. \
         Send required_lecturer_count >= 1 to have the solver choose, or send no candidates \
         at all for a genuinely unstaffed Offering"
    )]
    ZeroLecturersRequiredWithCandidates { offering: String, candidates: usize },

    // -- sessions ------------------------------------------------------------
    /// A slotless Session realizing NO Offering.
    ///
    /// Slotless is legitimate on its own — that is the spare bank (issue #22),
    /// teaching that is owed but unplaced. What cannot be interpreted is
    /// slotless AND ownerless: unlike an ad-hoc PLACED Session (a
    /// `staff_meeting`, which is real occupancy), this one owes teaching to
    /// nothing and no run under any scope or policy could ever place it.
    #[error(
        "session '{session}' has no start_slot and no offering_id; an unplaced Session is the \
         spare bank, which only means something for a Session that realizes an Offering — this \
         one owes teaching to nothing and no run could place it"
    )]
    SessionWithoutStart { session: String },
    #[error(
        "session '{session}' sits at week {week} day {day} block {block}, which is not a slot \
         in this tenant's grid"
    )]
    SessionOffGrid {
        session: String,
        week: u32,
        day: u32,
        block: u32,
    },
    #[error("external_occupancy entry has no start_slot")]
    ExternalOccupancyWithoutStart,
    #[error(
        "external_occupancy references room '{room}', which is not Federation-owned; occupancy \
         from another tenant only makes sense against a shared room"
    )]
    ExternalOccupancyOnPrivateRoom { room: String },

    // -- offerings -----------------------------------------------------------
    #[error("offering '{offering}' has duration_blocks = 0")]
    ZeroDurationOffering { offering: String },
    #[error(
        "offering '{offering}' has required_room_count {required}; the solver supports at most \
         {max} Rooms per Session"
    )]
    TooManyRoomsRequired {
        offering: String,
        required: u32,
        max: u32,
    },
    #[error(
        "offering '{offering}' has required_lecturer_count {required}; the solver supports at \
         most {max} lecturers per Session"
    )]
    TooManyLecturersRequired {
        offering: String,
        required: u32,
        max: u32,
    },
    #[error(
        "offering '{offering}' needs {required} lecturer(s) but names only {candidates} \
         candidate(s); a pool cannot be smaller than what it must supply"
    )]
    InsufficientLecturerCandidates {
        offering: String,
        required: u32,
        candidates: usize,
    },
    #[error(
        "offering '{offering}' has a genuine lecturer pool, but constraint '{constraint}' \
         (LecturerVeto) covers its kind; a pool Offering's veto mask cannot be precomputed \
         before the search chooses who leads each Session"
    )]
    LecturerVetoUnsupportedWithPool {
        offering: String,
        constraint: String,
    },

    // -- offering relations ---------------------------------------------------
    #[error(
        "relation '{relation}' names {members} Offering(s); a relation needs at least 2 to mean \
         anything"
    )]
    RelationTooFewMembers { relation: String, members: usize },
    #[error("relation '{relation}' has no params set")]
    RelationWithoutParams { relation: String },

    // -- constraints ---------------------------------------------------------
    #[error("constraint '{constraint}' has no params set")]
    ConstraintWithoutParams { constraint: String },
    #[error(
        "constraint '{constraint}' has max_ratio {ratio}; it is a share and must be in 0.0..=1.0"
    )]
    ShareRatioOutOfRange { constraint: String, ratio: f64 },
    #[error(
        "constraint '{constraint}' must set window to PER_TERM or PER_WEEK; the ratio is \
         meaningless without a window to measure it over"
    )]
    ShareWindowUnset { constraint: String },
    #[error("constraint '{constraint}': {day} is not an ISO weekday (1..=7)")]
    NotAnIsoWeekday { constraint: String, day: u32 },
    #[error(
        "constraint '{constraint}': MinimizeBlockUsage selects no blocks — set at least one \
         index, or `first`/`last`"
    )]
    BlockUsageSelectsNothing { constraint: String },
    #[error(
        "constraint '{constraint}' has weight {weight}; soft weights must be >= 0 because every \
         soft type declares minimize, and a negative weight would invert it"
    )]
    NegativeSoftWeight { constraint: String, weight: f64 },
    #[error(
        "scope.minimize_movement_weight is {weight}; it must be >= 0 for the same reason every \
         other soft weight must be — LOCK_POLICY_MINIMIZE_MOVEMENT declares minimize, and a \
         negative weight would invert it"
    )]
    NegativeMovementWeight { weight: f64 },
    #[error(
        "scope.minimize_inscope_movement_weight is {weight}; it must be >= 0 for the same reason \
         every other soft weight must be — it declares minimize, and a negative weight would \
         invert it"
    )]
    NegativeInScopeMovementWeight { weight: f64 },
    #[error(
        "scope.outside_scope_policy must be set; supported values are LOCK_POLICY_HARD and \
         LOCK_POLICY_MINIMIZE_MOVEMENT"
    )]
    LockPolicyUnset,
    #[error(
        "scope.movement_overrides[{index}] has weight {weight}; it must be >= 0 for the same \
         reason every other movement weight must be — it declares minimize, and a negative \
         weight would reward moving the very Sessions it was sent to protect"
    )]
    NegativeMovementOverrideWeight { index: usize, weight: f64 },
    #[error(
        "scope.movement_overrides[{index}] sets neither person_id nor group_id; an override with \
         no target cannot apply to anything, and silently dropping it would report a run as \
         respecting a protection it never had"
    )]
    MovementOverrideWithoutTarget { index: usize },
    #[error(
        "session '{session}' has no start_slot and is_locked; a lock on an unplaced Session has \
         two opposite readings — 'cancelled, never reschedule it' and 'meaningless, there is \
         nothing to lock' — so it is refused rather than guessed. Send it unlocked to bank the \
         teaching as owed, or keep it out of existing_sessions to drop the obligation"
    )]
    BankedSessionIsLocked { session: String },

    // -- deliberately not built yet ------------------------------------------
    //
    // Distinguished from the validation faults above because the caller cannot
    // fix these by correcting their data — the feature does not exist.
    #[error(
        "constraint '{constraint}': person_preference_fit counts lecturers' preferences only; \
         scoping it to role_tags {roles:?} is not implemented"
    )]
    PreferenceRolesUnsupported {
        constraint: String,
        roles: Vec<String>,
    },
    #[error(
        "constraint '{constraint}' uses type {constraint_type}, which is in the schema but has \
         no solver evaluator yet"
    )]
    ConstraintTypeUnimplemented {
        constraint: String,
        constraint_type: &'static str,
    },
    #[error(
        "relation '{relation}' uses kind {relation_kind}, which is in the schema but has no \
         solver evaluator yet"
    )]
    RelationKindUnimplemented {
        relation: String,
        relation_kind: &'static str,
    },
}

impl ConvertError {
    /// Whether this is an unbuilt feature rather than bad input.
    ///
    /// Keeping the rule as one predicate is what makes it reviewable.
    pub fn is_unimplemented(&self) -> bool {
        matches!(
            self,
            Self::PreferenceRolesUnsupported { .. }
                | Self::ConstraintTypeUnimplemented { .. }
                | Self::RelationKindUnimplemented { .. }
                | Self::LecturerVetoUnsupportedWithPool { .. }
        )
    }
}

impl From<ConvertError> for Status {
    /// The **only** place a conversion fault becomes a transport response.
    fn from(e: ConvertError) -> Self {
        let message = e.to_string();
        if e.is_unimplemented() {
            Status::unimplemented(message)
        } else {
            Status::invalid_argument(message)
        }
    }
}

/// Resolve a caller-supplied id to a dense index, declaring the policy in the
/// call.
///
/// The conversion module's own doc comment claimed it was "deliberately strict
/// about *structural* problems (a Session on a day the tenant does not teach, a
/// room id that does not exist)". It was not. Four sites silently dropped
/// unknown ids via `filter_map` while two hard-errored, with no module stating
/// which was which — and one of the silent drops turned a bad `room_id` into
/// **roomless occupancy**, structurally invisible to room double-booking.
///
/// There is a legitimate permissive case: a Session naming an Offering absent
/// from the snapshot is occupancy either way, and the caller's "warn and allow"
/// editing UX produces it. The problem was never that permissiveness existed —
/// it was that strict and permissive were chosen ad hoc per call site. Now every
/// call names its choice, and `grep` yields the whole policy.
pub struct Resolver<'a> {
    index: &'a std::collections::HashMap<String, u32>,
}

impl<'a> Resolver<'a> {
    pub fn new(index: &'a std::collections::HashMap<String, u32>) -> Self {
        Self { index }
    }

    /// The id must exist. Used where a dangling reference would silently change
    /// what the solver enforces.
    pub fn require<T>(
        &self,
        id: &str,
        wrap: fn(u32) -> T,
        missing: impl FnOnce(String) -> ConvertError,
    ) -> Result<T, ConvertError> {
        match self.index.get(id) {
            Some(&i) => Ok(wrap(i)),
            None => Err(missing(id.to_string())),
        }
    }

    /// Every id must exist.
    pub fn require_all<T>(
        &self,
        ids: &[String],
        wrap: fn(u32) -> T,
        missing: impl Fn(String) -> ConvertError,
    ) -> Result<Vec<T>, ConvertError> {
        ids.iter()
            .map(|id| self.require(id, wrap, &missing))
            .collect()
    }

    /// The id may be absent, and its absence is meaningful rather than an error.
    /// Each use must say why in a comment at the call site.
    pub fn optional<T>(&self, id: &str, wrap: fn(u32) -> T) -> Option<T> {
        self.index.get(id).map(|&i| wrap(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_faults_map_to_invalid_argument() {
        let status: Status = ConvertError::MissingTimeGrid.into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn a_negative_movement_weight_is_invalid_argument_not_unimplemented() {
        // LOCK_POLICY_MINIMIZE_MOVEMENT is built; a negative weight is bad
        // data the caller can fix, not an absent feature.
        let status: Status = ConvertError::NegativeMovementWeight { weight: -1.0 }.into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn a_lecturer_pool_fault_is_invalid_argument_not_unimplemented() {
        // Lecturer-pool selection is built; too few candidates or too large a
        // pool is bad data the caller can fix, not an absent feature.
        let status: Status = ConvertError::InsufficientLecturerCandidates {
            offering: "o1".into(),
            required: 2,
            candidates: 1,
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let status: Status =
            ConvertError::TooManyLecturersRequired { offering: "o1".into(), required: 99, max: 4 }
                .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn a_group_cycle_survives_the_boundary_as_a_variant() {
        use calendry_solver_core::ids::GroupIdx;
        // RED against the old code, which called `.to_string()` on the cycle and
        // kept only the prose.
        let e = ConvertError::from(GroupCycle(vec![GroupIdx(0), GroupIdx(1)]));
        assert!(matches!(e, ConvertError::GroupCycle(_)));
        assert_eq!(Status::from(e).code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn require_reports_the_missing_id() {
        let index = std::collections::HashMap::new();
        let r = Resolver::new(&index);
        let err = r
            .require("r9", calendry_solver_core::ids::RoomIdx, |room| ConvertError::UnknownRoom {
                context: "session 's1'".into(),
                room,
            })
            .expect_err("unknown id must be refused");
        assert!(err.to_string().contains("r9"), "{err}");
    }

    #[test]
    fn optional_returns_none_without_erroring() {
        let index = std::collections::HashMap::new();
        let r = Resolver::new(&index);
        assert!(
            r.optional("nope", calendry_solver_core::ids::RoomIdx)
                .is_none()
        );
    }
}
