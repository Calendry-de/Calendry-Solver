//! Wire-shaped fixtures for the service's integration tests.
//!
//! These build `pb::SolverInput` values, which is a different job from
//! `calendry_solver_core::testing` — that assembles `Problem` values directly
//! and never sees a prost type. The two do not share a source of truth, and
//! should not: the whole point of these tests is to exercise the boundary
//! between the two representations.

// Each test binary uses a different subset of this module, so `dead_code` fires
// on whatever a given binary does not call. A property of how cargo builds
// integration tests, not of this file.
#![allow(dead_code, unreachable_pub)]

use calendry_solver_proto::v1 as pb;

pub const KIND: &str = "lecture";

pub fn slot(week: u32, day: u32, block: u32) -> pb::SlotRef {
    pb::SlotRef { week, day, block }
}

/// Roomy enough that nothing here is infeasible for want of space:
/// 4 weeks x Mon-Fri x 6 blocks = 120 slots, 2 rooms.
pub fn base_input() -> pb::SolverInput {
    pb::SolverInput {
        requesting_tenant_id: "t1".into(),
        federation_id: String::new(),
        time_grid: Some(pb::TimeGrid {
            blocks_per_day: 6,
            block_length_minutes: 45,
            day_start_minute: 480,
            active_days: vec![1, 2, 3, 4, 5],
            institution_timezone: "Europe/Berlin".into(),
        }),
        calendar: Some(pb::AcademicCalendar {
            term_id: "term-1".into(),
            weeks: (0..4)
                .map(|i| pb::Week {
                    index: i,
                    start_date: format!("2026-01-{:02}", 5 + i * 7),
                    kind: pb::WeekKind::Teaching as i32,
                })
                .collect(),
            holidays: vec![],
        }),
        rooms: (0..2).map(room).collect(),
        persons: vec![person("p1")],
        groups: vec![group("g1")],
        offerings: vec![],
        existing_sessions: vec![],
        external_occupancy: vec![],
        constraints: vec![
            enabled(
                "c-room",
                pb::constraint_config::Params::RoomDoubleBooking(pb::RoomDoubleBooking {}),
            ),
            enabled("c-freq", pb::constraint_config::Params::ExactFrequency(pb::ExactFrequency {})),
        ],
        // Week 0 Monday block 0 — nothing here is past unless a test puts it
        // there deliberately.
        reference_slot: Some(slot(0, 1, 0)),
    }
}

pub fn room(i: u32) -> pb::Room {
    pb::Room {
        id: format!("r{i}"),
        owner: Some(pb::room::Owner::TenantId("t1".into())),
        name: format!("Room {i}"),
        capacity: 100,
        rank: 1,
        is_virtual: false,
        feature_tags: vec![],
        location: String::new(),
    }
}

pub fn federation_room(i: u32) -> pb::Room {
    pb::Room { owner: Some(pb::room::Owner::FederationId("f1".into())), ..room(i) }
}

pub fn person(id: &str) -> pb::Person {
    pb::Person {
        id: id.into(),
        role_tags: vec!["Lecturer".into()],
        group_ids: vec![],
        blackouts: vec![],
        // Schema 0.7.0. `None` is the "no stated preference" case, which is what
        // these fixtures mean: the rule is off unless a test enables it, so a
        // preference here would be data nothing reads. See
        // `tests/person_preference_wire.rs`, which supplies its own.
        preferred: None,
    }
}

pub fn group(id: &str) -> pb::Group {
    pb::Group { id: id.into(), parent_id: String::new(), name: format!("Group {id}"), size: 20 }
}

pub fn enabled(id: &str, params: pb::constraint_config::Params) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        id: id.into(),
        enabled: true,
        applies_to_kinds: vec![],
        weight: 0.0,
        params: Some(params),
    }
}

pub fn offering(id: &str, required: u32) -> pb::Offering {
    pb::Offering {
        id: id.into(),
        owner: Some(pb::offering::Owner::TenantId("t1".into())),
        kind: KIND.into(),
        required_session_count: required,
        duration_blocks: 1,
        candidate_lecturer_ids: vec!["p1".into()],
        required_lecturer_count: 1,
        group_ids: vec!["g1".into()],
        participant_person_ids: vec![],
        required_room_features: vec![],
        min_capacity: 0,
        allowed_room_ids: vec![],
        allow_online: false,
    }
}

pub fn session(id: &str, offering_id: &str, at: pb::SlotRef) -> pb::Session {
    pb::Session {
        id: id.into(),
        owner: Some(pb::session::Owner::TenantId("t1".into())),
        offering_id: offering_id.into(),
        kind: KIND.into(),
        start_slot: Some(at),
        duration_blocks: 1,
        room_id: "r0".into(),
        lecturer_ids: vec!["p1".into()],
        group_ids: vec!["g1".into()],
        person_ids: vec![],
        is_locked: false,
    }
}

pub fn locked_session(id: &str, offering_id: &str, at: pb::SlotRef) -> pb::Session {
    pb::Session { is_locked: true, ..session(id, offering_id, at) }
}

pub fn scope(ids: &[&str]) -> pb::SolveScope {
    pb::SolveScope {
        offering_ids: ids.iter().map(|s| (*s).to_string()).collect(),
        group_ids: vec![],
        outside_scope_policy: pb::LockPolicy::Hard as i32,
    }
}

/// A single-slot grid: one week, Monday only, one block. Two rooms.
pub fn one_slot_grid(input: &mut pb::SolverInput) {
    input.time_grid = Some(pb::TimeGrid {
        blocks_per_day: 1,
        block_length_minutes: 45,
        day_start_minute: 480,
        active_days: vec![1],
        institution_timezone: "Europe/Berlin".into(),
    });
    input.calendar = Some(pb::AcademicCalendar {
        term_id: "term-1".into(),
        weeks: vec![pb::Week {
            index: 0,
            start_date: "2026-01-05".into(),
            kind: pb::WeekKind::Teaching as i32,
        }],
        holidays: vec![],
    });
}
