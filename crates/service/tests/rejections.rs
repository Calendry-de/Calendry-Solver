//! What the conversion boundary refuses, and with which gRPC code.
//!
//! Before the service crate had a library target, **not one** of these paths had
//! a test: there were 21 `tonic::Status` construction sites in `convert.rs` and
//! nothing could link them. Two things had to change first — the library target,
//! and `ConvertError`, which is what lets these assert on a **variant** rather
//! than on message prose.
//!
//! Each test names the fault it is checking, and the ones marked RED were
//! written against the old permissive behaviour and confirmed failing before the
//! resolution policy changed.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;
use tonic::Code;

mod common;
use common::{
    base_input, enabled, federation_room, group, locked_session, offering, scope, session, slot,
};

/// Convert and expect a refusal.
fn reject(input: &pb::SolverInput, scope: &pb::SolveScope) -> ConvertError {
    match convert(input, scope) {
        Ok(_) => panic!("this input must be refused"),
        Err(e) => e,
    }
}

fn code_of(e: &ConvertError) -> Code {
    // `ConvertError` is consumed by the `From` impl, so re-derive from the same
    // predicate rather than cloning an error that carries a `GroupCycle`.
    if e.is_unimplemented() { Code::Unimplemented } else { Code::InvalidArgument }
}

// ---------------------------------------------------------------------------
// Required fields
// ---------------------------------------------------------------------------

#[test]
fn a_missing_time_grid_is_refused() {
    let mut input = base_input();
    input.time_grid = None;

    let e = reject(&input, &scope(&[]));
    assert!(matches!(e, ConvertError::MissingTimeGrid), "{e}");
    assert_eq!(code_of(&e), Code::InvalidArgument);
}

#[test]
fn a_missing_calendar_is_refused() {
    let mut input = base_input();
    input.calendar = None;

    assert!(matches!(reject(&input, &scope(&[])), ConvertError::MissingCalendar));
}

#[test]
fn a_grid_with_no_active_days_is_refused() {
    let mut input = base_input();
    input.time_grid = Some(pb::TimeGrid { active_days: vec![], ..input.time_grid.unwrap() });

    let e = reject(&input, &scope(&[]));
    assert!(matches!(e, ConvertError::InvalidTimeGrid { .. }), "{e}");
}

// ---------------------------------------------------------------------------
// Lock policy
// ---------------------------------------------------------------------------

#[test]
fn a_negative_minimize_movement_weight_is_refused() {
    let mut s = scope(&[]);
    s.outside_scope_policy = pb::LockPolicy::MinimizeMovement as i32;
    s.minimize_movement_weight = -1.0;

    let e = reject(&base_input(), &s);
    assert!(matches!(e, ConvertError::NegativeMovementWeight { weight } if weight == -1.0), "{e}");
    assert_eq!(
        code_of(&e),
        Code::InvalidArgument,
        "the caller can fix this by sending a non-negative weight"
    );
}

#[test]
fn a_nan_minimize_movement_weight_is_refused() {
    let mut s = scope(&[]);
    s.outside_scope_policy = pb::LockPolicy::MinimizeMovement as i32;
    s.minimize_movement_weight = f64::NAN;

    let e = reject(&base_input(), &s);
    assert!(matches!(e, ConvertError::NegativeMovementWeight { weight } if weight.is_nan()), "{e}");
}

#[test]
fn an_unset_lock_policy_is_refused() {
    let mut s = scope(&[]);
    s.outside_scope_policy = 0;

    let e = reject(&base_input(), &s);
    assert!(matches!(e, ConvertError::LockPolicyUnset), "{e}");
    assert_eq!(code_of(&e), Code::InvalidArgument);
}

// ---------------------------------------------------------------------------
// Entity references
// ---------------------------------------------------------------------------

#[test]
fn a_group_naming_an_unknown_parent_is_refused() {
    let mut input = base_input();
    input.groups = vec![pb::Group { parent_id: "nope".into(), ..group("g1") }];

    let e = reject(&input, &scope(&[]));
    assert!(
        matches!(&e, ConvertError::UnknownGroupParent { group, parent }
                     if group == "g1" && parent == "nope"),
        "{e}"
    );
}

#[test]
fn a_cyclic_group_hierarchy_survives_the_boundary_as_a_variant() {
    // RED against the old code, which did `.map_err(|c| c.to_string())` and kept
    // only the prose, so a caller had to match on message text to tell a cycle
    // apart from a malformed grid.
    let mut input = base_input();
    input.groups = vec![
        pb::Group { id: "a".into(), parent_id: "b".into(), ..group("a") },
        pb::Group { id: "b".into(), parent_id: "a".into(), ..group("b") },
    ];
    input.offerings = vec![pb::Offering { group_ids: vec!["a".into()], ..offering("o1", 1) }];

    let e = reject(&input, &scope(&["o1"]));
    assert!(matches!(e, ConvertError::GroupCycle(_)), "{e}");
}

#[test]
fn an_offering_naming_an_unknown_lecturer_is_refused() {
    // RED against the old `filter_map`, which silently dropped the id. The
    // Offering would then have been solved with fewer lecturers than the caller
    // assigned — and the `required_lecturer_count` gate still passed, because it
    // was checked against the *wire* list before resolution.
    let mut input = base_input();
    input.offerings =
        vec![pb::Offering { candidate_lecturer_ids: vec!["ghost".into()], ..offering("o1", 1) }];

    let e = reject(&input, &scope(&["o1"]));
    assert!(matches!(&e, ConvertError::UnknownPerson { person, .. } if person == "ghost"), "{e}");
}

#[test]
fn an_offering_naming_an_unknown_group_is_refused() {
    let mut input = base_input();
    input.offerings = vec![pb::Offering { group_ids: vec!["ghost".into()], ..offering("o1", 1) }];

    let e = reject(&input, &scope(&["o1"]));
    assert!(matches!(&e, ConvertError::UnknownGroup { group, .. } if group == "ghost"), "{e}");
}

#[test]
fn a_person_in_an_unknown_group_is_refused() {
    // RED against the old `filter_map`. A silently dropped membership removes the
    // Person from that Group's attendee list, so `PersonDoubleBooking` stops
    // seeing a clash that is really there.
    let mut input = base_input();
    input.persons = vec![pb::Person { group_ids: vec!["ghost".into()], ..common::person("p1") }];

    let e = reject(&input, &scope(&[]));
    assert!(matches!(&e, ConvertError::UnknownGroup { group, .. } if group == "ghost"), "{e}");
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[test]
fn a_session_naming_an_unknown_room_is_refused() {
    // RED against the old behaviour, and the sharpest of these: an unknown
    // `room_id` resolved to `None`, which made the Session **roomless
    // occupancy**. It still blocked its lecturers and groups, but room
    // double-booking structurally could not see it, so the solver would happily
    // place another Session into a room a locked Session was already using.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions =
        vec![pb::Session { room_id: "ghost".into(), ..locked_session("s1", "o1", slot(0, 1, 1)) }];

    let e = reject(&input, &scope(&["o1"]));
    assert!(matches!(&e, ConvertError::UnknownRoom { room, .. } if room == "ghost"), "{e}");
}

#[test]
fn a_session_with_no_room_at_all_is_still_accepted() {
    // The distinction that matters: an *unknown* room is a dangling reference,
    // but an *empty* room id is a real state — an online-only or not-yet-roomed
    // Session. Refusing both would break "warn and allow".
    let mut input = base_input();
    input.offerings = vec![offering("o1", 2)];
    input.existing_sessions =
        vec![pb::Session { room_id: String::new(), ..locked_session("s1", "o1", slot(0, 1, 1)) }];

    let problem = convert(&input, &scope(&["o1"])).expect("an unroomed Session is legitimate");
    assert_eq!(problem.fixed.len(), 1);
    assert!(problem.fixed[0].room.is_none());
}

#[test]
fn a_session_with_no_start_slot_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions =
        vec![pb::Session { start_slot: None, ..session("s1", "o1", slot(0, 1, 1)) }];

    let e = reject(&input, &scope(&["o1"]));
    assert!(matches!(&e, ConvertError::SessionWithoutStart { session } if session == "s1"), "{e}");
}

#[test]
fn a_session_off_the_tenants_grid_is_refused() {
    // Saturday, on a Mon-Fri grid.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![session("s1", "o1", slot(0, 6, 0))];

    let e = reject(&input, &scope(&["o1"]));
    assert!(
        matches!(&e, ConvertError::SessionOffGrid { session, day, .. } if session == "s1" && *day == 6),
        "{e}"
    );
}

#[test]
fn a_session_naming_an_offering_absent_from_the_snapshot_is_accepted() {
    // The documented permissive case, and the reason `Resolver::optional` exists:
    // this is occupancy either way, and the caller's editing UX produces it.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![locked_session("s1", "gone", slot(0, 1, 1))];

    let problem = convert(&input, &scope(&["o1"])).expect("occupancy without an Offering is fine");
    assert_eq!(problem.fixed.len(), 1);
    assert!(problem.fixed[0].offering.is_none());
}

// ---------------------------------------------------------------------------
// External occupancy
// ---------------------------------------------------------------------------

#[test]
fn external_occupancy_on_an_unknown_room_is_refused() {
    let mut input = base_input();
    input.external_occupancy = vec![pb::ExternalOccupancy {
        room_id: "ghost".into(),
        start_slot: Some(slot(0, 1, 0)),
        duration_blocks: 1,
        source_ref: String::new(),
    }];

    let e = reject(&input, &scope(&[]));
    assert!(matches!(&e, ConvertError::UnknownRoom { room, .. } if room == "ghost"), "{e}");
}

#[test]
fn external_occupancy_on_a_tenant_owned_room_is_refused() {
    // Occupancy from another tenant only makes sense against a Federation-shared
    // Room. Against a private one it is a modelling error, not a conflict.
    let mut input = base_input();
    input.external_occupancy = vec![pb::ExternalOccupancy {
        room_id: "r0".into(),
        start_slot: Some(slot(0, 1, 0)),
        duration_blocks: 1,
        source_ref: String::new(),
    }];

    let e = reject(&input, &scope(&[]));
    assert!(matches!(e, ConvertError::ExternalOccupancyOnPrivateRoom { .. }), "{e}");
}

#[test]
fn external_occupancy_on_a_federation_room_is_accepted() {
    let mut input = base_input();
    input.rooms = vec![federation_room(0), common::room(1)];
    input.external_occupancy = vec![pb::ExternalOccupancy {
        room_id: "r0".into(),
        start_slot: Some(slot(0, 1, 0)),
        duration_blocks: 2,
        source_ref: "other-tenant-booking".into(),
    }];

    let problem = convert(&input, &scope(&[])).expect("a shared room may carry external use");
    assert_eq!(problem.fixed.len(), 1);
    assert_eq!(problem.fixed[0].session_id, "external:other-tenant-booking");
}

#[test]
fn external_occupancy_with_no_start_slot_is_refused() {
    let mut input = base_input();
    input.rooms = vec![federation_room(0)];
    input.external_occupancy = vec![pb::ExternalOccupancy {
        room_id: "r0".into(),
        start_slot: None,
        duration_blocks: 1,
        source_ref: String::new(),
    }];

    assert!(matches!(reject(&input, &scope(&[])), ConvertError::ExternalOccupancyWithoutStart));
}

// ---------------------------------------------------------------------------
// Offerings
// ---------------------------------------------------------------------------

#[test]
fn a_zero_duration_offering_is_refused() {
    let mut input = base_input();
    input.offerings = vec![pb::Offering { duration_blocks: 0, ..offering("o1", 1) }];

    let e = reject(&input, &scope(&["o1"]));
    assert!(
        matches!(&e, ConvertError::ZeroDurationOffering { offering } if offering == "o1"),
        "{e}"
    );
}

#[test]
fn a_genuine_lecturer_pool_is_unimplemented() {
    // v1 takes lecturers as already assigned; choosing among candidates is a
    // materially larger search space. Refused rather than silently mis-solved.
    let mut input = base_input();
    input.persons = vec![common::person("p1"), common::person("p2")];
    input.offerings = vec![pb::Offering {
        candidate_lecturer_ids: vec!["p1".into(), "p2".into()],
        required_lecturer_count: 1,
        ..offering("o1", 1)
    }];

    let e = reject(&input, &scope(&["o1"]));
    assert!(matches!(e, ConvertError::LecturerPoolUnsupported { .. }), "{e}");
    assert_eq!(code_of(&e), Code::Unimplemented);
}

// ---------------------------------------------------------------------------
// Constraints
// ---------------------------------------------------------------------------

#[test]
fn a_constraint_with_no_params_is_refused() {
    let mut input = base_input();
    input.constraints.push(pb::ConstraintConfig {
        id: "c-empty".into(),
        enabled: true,
        applies_to_kinds: vec![],
        weight: 0.0,
        params: None,
    });

    let e = reject(&input, &scope(&[]));
    assert!(
        matches!(&e, ConvertError::ConstraintWithoutParams { constraint } if constraint == "c-empty"),
        "{e}"
    );
}

#[test]
fn a_negative_compactness_weight_is_refused() {
    // Compactness is built now (see tests/compactness.rs for the substantive
    // behavior) — the remaining refusal is the same validation fault every
    // other soft weight gets.
    let mut input = base_input();
    input.constraints.push(enabled(
        "c-compact",
        pb::constraint_config::Params::Compactness(pb::Compactness { scope: vec![] }),
    ));
    input.constraints.last_mut().unwrap().weight = -1.0;

    let e = reject(&input, &scope(&[]));
    assert!(
        matches!(&e, ConvertError::NegativeSoftWeight { constraint, weight } if constraint == "c-compact" && *weight == -1.0),
        "{e}"
    );
    assert_eq!(code_of(&e), Code::InvalidArgument);
}

#[test]
fn lecturer_consistency_is_unimplemented_not_invalid() {
    let mut input = base_input();
    input.constraints.push(enabled(
        "c-consistent",
        pb::constraint_config::Params::LecturerConsistency(pb::LecturerConsistency {}),
    ));

    let e = reject(&input, &scope(&[]));
    assert!(
        matches!(
            &e,
            ConvertError::ConstraintTypeUnimplemented { constraint, constraint_type }
                if constraint == "c-consistent" && *constraint_type == "LecturerConsistency"
        ),
        "{e}"
    );
    assert_eq!(code_of(&e), Code::Unimplemented);
}

/// The P2 batch, staged together for one proto version bump: every type
/// refuses as UNIMPLEMENTED except `GroupSizeFitsRoom` and
/// `MaxConcurrentOnlineSessions` (each covered by their own test file, not
/// here) — one test per type rather than a dozen near-identical functions.
#[test]
fn every_staged_p2_type_is_unimplemented_not_invalid() {
    use pb::constraint_config::Params;

    let cases: Vec<(&str, Params)> = vec![
        (
            "MinimizeLocationChange",
            Params::MinimizeLocationChange(pb::MinimizeLocationChange::default()),
        ),
        (
            "MaxWeeklyTeachingLoad",
            Params::MaxWeeklyTeachingLoad(pb::MaxWeeklyTeachingLoad::default()),
        ),
        ("ExamSpacingSameDay", Params::ExamSpacingSameDay(pb::ExamSpacingSameDay {})),
        ("ExamSpacingWindow", Params::ExamSpacingWindow(pb::ExamSpacingWindow::default())),
        ("RoomConsistency", Params::RoomConsistency(pb::RoomConsistency {})),
        ("MinimizeRoomChurn", Params::MinimizeRoomChurn(pb::MinimizeRoomChurn::default())),
        ("MaxDailySpan", Params::MaxDailySpan(pb::MaxDailySpan::default())),
        (
            "MinimizeWeekdayImbalance",
            Params::MinimizeWeekdayImbalance(pb::MinimizeWeekdayImbalance::default()),
        ),
        (
            "RoomTurnaroundBuffer",
            Params::RoomTurnaroundBuffer(pb::RoomTurnaroundBuffer::default()),
        ),
    ];

    for (name, params) in cases {
        let mut input = base_input();
        input.constraints.push(enabled("c-staged", params));

        let e = reject(&input, &scope(&[]));
        assert!(
            matches!(
                &e,
                ConvertError::ConstraintTypeUnimplemented { constraint, constraint_type }
                    if constraint == "c-staged" && *constraint_type == name
            ),
            "{name}: {e}"
        );
        assert_eq!(code_of(&e), Code::Unimplemented, "{name}");
    }
}

#[test]
fn a_disabled_constraint_with_no_params_is_ignored() {
    // Only *enabled* constraints are read, so an unfinished row in the tenant's
    // config is not a reason to reject the whole snapshot.
    let mut input = base_input();
    input.constraints.push(pb::ConstraintConfig {
        id: "c-empty".into(),
        enabled: false,
        applies_to_kinds: vec![],
        weight: 0.0,
        params: None,
    });

    convert(&input, &scope(&[])).expect("a disabled constraint is not read");
}

#[test]
fn a_share_ratio_outside_zero_to_one_is_refused() {
    for ratio in [-0.1, 1.5, f64::NAN] {
        let mut input = base_input();
        input.constraints.push(enabled(
            "c-share",
            pb::constraint_config::Params::MaxOnlineShare(pb::MaxOnlineShare {
                max_ratio: ratio,
                window: pb::ShareWindow::PerTerm as i32,
            }),
        ));

        let e = reject(&input, &scope(&[]));
        assert!(
            matches!(e, ConvertError::ShareRatioOutOfRange { .. }),
            "ratio {ratio} must be refused, got {e}"
        );
    }
}

#[test]
fn a_share_cap_with_no_window_is_refused() {
    // A ratio is meaningless without a window to measure it over.
    let mut input = base_input();
    input.constraints.push(enabled(
        "c-share",
        pb::constraint_config::Params::MaxOnlineShare(pb::MaxOnlineShare {
            max_ratio: 0.3,
            window: 0,
        }),
    ));

    let e = reject(&input, &scope(&[]));
    assert!(
        matches!(&e, ConvertError::ShareWindowUnset { constraint } if constraint == "c-share"),
        "{e}"
    );
}

#[test]
fn a_day_outside_the_iso_range_is_refused() {
    for day in [0u32, 8, 99] {
        let mut input = base_input();
        input.constraints.push(enabled(
            "c-day",
            pb::constraint_config::Params::MinimizeDayUsage(pb::MinimizeDayUsage {
                days: vec![day],
            }),
        ));

        let e = reject(&input, &scope(&[]));
        assert!(
            matches!(&e, ConvertError::NotAnIsoWeekday { day: d, .. } if *d == day),
            "day {day} must be refused, got {e}"
        );
    }
}

#[test]
fn a_negative_soft_weight_is_refused() {
    // Every soft type declares "minimize". A negative weight would silently
    // invert it into a maximize the type never declared.
    let mut input = base_input();
    let mut c = enabled(
        "c-first",
        pb::constraint_config::Params::MinimizeBlockUsage(pb::MinimizeBlockUsage {
            blocks: vec![],
            first: true,
            last: false,
        }),
    );
    c.weight = -1.0;
    input.constraints.push(c);

    let e = reject(&input, &scope(&[]));
    assert!(
        matches!(&e, ConvertError::NegativeSoftWeight { constraint, .. } if constraint == "c-first"),
        "{e}"
    );
}

#[test]
fn a_zero_soft_weight_is_accepted() {
    // Zero means "report the count, do not steer" — a legitimate configuration.
    let mut input = base_input();
    input.constraints.push(enabled(
        "c-first",
        pb::constraint_config::Params::MinimizeBlockUsage(pb::MinimizeBlockUsage {
            blocks: vec![],
            first: true,
            last: false,
        }),
    ));

    convert(&input, &scope(&[])).expect("zero weight is meaningful, not an error");
}

// ---------------------------------------------------------------------------
// The transport mapping
// ---------------------------------------------------------------------------

#[test]
fn every_refusal_maps_to_invalid_argument_or_unimplemented_and_nothing_else() {
    // The property the scattered version could not state: exactly two codes come
    // out of this boundary, and which one is decided in a single predicate.
    let cases: Vec<ConvertError> = vec![
        ConvertError::MissingTimeGrid,
        ConvertError::MissingCalendar,
        ConvertError::MissingInput,
        ConvertError::MissingScope,
        ConvertError::InvalidTimeGrid { reason: "x".into() },
        ConvertError::UnknownGroupParent { group: "g".into(), parent: "p".into() },
        ConvertError::UnknownRoom { context: "c".into(), room: "r".into() },
        ConvertError::UnknownPerson { context: "c".into(), person: "p".into() },
        ConvertError::UnknownGroup { context: "c".into(), group: "g".into() },
        ConvertError::SessionWithoutStart { session: "s".into() },
        ConvertError::SessionOffGrid { session: "s".into(), week: 0, day: 6, block: 0 },
        ConvertError::ExternalOccupancyWithoutStart,
        ConvertError::ExternalOccupancyOnPrivateRoom { room: "r".into() },
        ConvertError::ZeroDurationOffering { offering: "o".into() },
        ConvertError::ConstraintWithoutParams { constraint: "c".into() },
        ConvertError::ShareRatioOutOfRange { constraint: "c".into(), ratio: 2.0 },
        ConvertError::ShareWindowUnset { constraint: "c".into() },
        ConvertError::NotAnIsoWeekday { constraint: "c".into(), day: 9 },
        ConvertError::NegativeSoftWeight { constraint: "c".into(), weight: -1.0 },
        ConvertError::NegativeMovementWeight { weight: -1.0 },
        ConvertError::LockPolicyUnset,
        ConvertError::LecturerPoolUnsupported { offering: "o".into(), required: 1, candidates: 3 },
        ConvertError::PreferenceRolesUnsupported {
            constraint: "c".into(),
            roles: vec!["Student".into()],
        },
        ConvertError::ConstraintTypeUnimplemented {
            constraint: "c".into(),
            constraint_type: "LecturerConsistency",
        },
    ];

    for e in cases {
        let text = e.to_string();
        assert!(!text.is_empty(), "every variant must render a message");
        let status = tonic::Status::from(e);
        assert!(
            matches!(status.code(), Code::InvalidArgument | Code::Unimplemented),
            "{text} produced {:?}",
            status.code()
        );
        assert_eq!(status.message(), text, "the message must survive the mapping");
    }
}
