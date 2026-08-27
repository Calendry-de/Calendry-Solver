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
    #[error(transparent)]
    GroupCycle(#[from] GroupCycle),

    // -- sessions ------------------------------------------------------------
    #[error("session '{session}' has no start_slot")]
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

    // -- deliberately not built yet ------------------------------------------
    //
    // Distinguished from the validation faults above because the caller cannot
    // fix these by correcting their data — the feature does not exist.
    #[error(
        "LOCK_POLICY_MINIMIZE_MOVEMENT is the deferred v2 policy; v1 hard-locks everything \
         outside scope"
    )]
    MinimizeMovementUnsupported,
    #[error("scope.outside_scope_policy must be set; v1 supports LOCK_POLICY_HARD")]
    LockPolicyUnset,
    #[error(
        "offering '{offering}' asks the solver to choose {required} of {candidates} candidate \
         lecturers; v1 supports pre-assigned lecturers only"
    )]
    LecturerPoolUnsupported {
        offering: String,
        required: u32,
        candidates: usize,
    },
    #[error(
        "constraint '{constraint}': person_preference_fit counts lecturers' preferences only; \
         scoping it to role_tags {roles:?} is not implemented"
    )]
    PreferenceRolesUnsupported {
        constraint: String,
        roles: Vec<String>,
    },
}

impl ConvertError {
    /// Whether this is an unbuilt feature rather than bad input.
    ///
    /// The distinction the scattered version had already lost: an unsupported
    /// lecturer pool is data the caller *can* change, but there is nothing they
    /// can send that v1 will solve, so `UNIMPLEMENTED` is still the honest code.
    /// Keeping the rule as one predicate is what makes it reviewable.
    pub fn is_unimplemented(&self) -> bool {
        matches!(
            self,
            Self::MinimizeMovementUnsupported
                | Self::LecturerPoolUnsupported { .. }
                | Self::PreferenceRolesUnsupported { .. }
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
    fn unbuilt_features_map_to_unimplemented() {
        let status: Status = ConvertError::MinimizeMovementUnsupported.into();
        assert_eq!(status.code(), tonic::Code::Unimplemented);
    }

    #[test]
    fn a_lecturer_pool_is_unimplemented_not_invalid_argument() {
        // There is no input the caller can send that v1 will solve, so this is
        // an absent feature rather than bad data.
        let status: Status = ConvertError::LecturerPoolUnsupported {
            offering: "o1".into(),
            required: 1,
            candidates: 3,
        }
        .into();
        assert_eq!(status.code(), tonic::Code::Unimplemented);
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
