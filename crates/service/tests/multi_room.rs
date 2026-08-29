//! `Offering.required_room_count` / `eligible_room_combinations`,
//! `Session.room_ids` / `PlacedSession.room_ids`: the wire shape for a
//! Session occupying more than one Room at once.
//!
//! The solver's placement primitive itself — marking, checking and costing
//! every Room in a combination together — is exercised in
//! `crates/core/tests/multi_room.rs`. These tests are the wire boundary:
//! combinatorial enumeration and capacity summing in `build_offerings`,
//! reading `room_ids` on input, writing it on output, and that an ordinary
//! `required_room_count <= 1` Offering is unaffected.

use calendry_solver::convert::{build_output, convert};
use calendry_solver::error::ConvertError;
use calendry_solver_core::ids::{OfferingIdx, RoomIdx};
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, offering, room, scope, session};

const SEED: u64 = 0xC0FFEE;

fn budget() -> Budget {
    Budget { max_wall_millis: 0, max_moves: 5_000 }
}

#[test]
fn capacity_sums_across_the_combination() {
    let mut input = base_input();
    input.rooms = vec![
        pb::Room { capacity: 60, ..room(0) },
        pb::Room { capacity: 50, ..room(1) },
        pb::Room { capacity: 10, ..room(2) },
    ];
    input.offerings =
        vec![pb::Offering { required_room_count: 2, min_capacity: 100, ..offering("o1", 1) }];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    let combos = &problem.offerings[OfferingIdx(0).get()].eligible_room_combinations;

    // Room 0 + Room 1 sum to 110 >= 100. Every other pair falls short: 0+2 is
    // 70, 1+2 is 60. Only the one combination whose SUMMED capacity clears
    // the bar may appear, regardless of any individual Room's own capacity.
    assert_eq!(combos.len(), 1, "exactly one pair sums to at least 100");
    let (primary, additional) = combos[0];
    let mut rooms = vec![primary];
    rooms.extend(additional.into_iter().flatten());
    rooms.sort_by_key(|r| r.get());
    assert_eq!(rooms, vec![RoomIdx(0), RoomIdx(1)]);
}

#[test]
fn too_many_rooms_required_is_refused() {
    let mut input = base_input();
    input.rooms = (0..6).map(room).collect();
    input.offerings = vec![pb::Offering { required_room_count: 5, ..offering("o1", 1) }];

    let e = convert(&input, &scope(&["o1"])).expect_err("5 Rooms exceeds the solver's cap of 4");
    assert!(matches!(e, ConvertError::TooManyRoomsRequired { required: 5, max: 4, .. }));
}

#[test]
fn room_ids_is_read_as_the_authoritative_set_on_input() {
    // An existing, locked Session already occupying two Rooms. `room_id`
    // alone would only ever resolve the primary; `room_ids` is what carries
    // the full set across the wire.
    let mut input = base_input();
    input.offerings = vec![pb::Offering { required_room_count: 2, ..offering("o1", 1) }];
    input.existing_sessions = vec![pb::Session {
        room_ids: vec!["r0".into(), "r1".into()],
        is_locked: true,
        ..session("s1", "o1", common::slot(0, 1, 0))
    }];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.fixed.len(), 1);
    let f = &problem.fixed[0];
    assert_eq!(f.room, Some(RoomIdx(0)), "room_id stays the primary Room");
    assert_eq!(
        f.additional_rooms[0],
        Some(RoomIdx(1)),
        "the entry in room_ids beyond room_id becomes an additional Room"
    );
}

#[test]
fn room_ids_is_written_on_output_only_for_a_multi_room_placement() {
    let mut input = base_input();
    input.rooms = (0..3).map(room).collect();
    input.offerings = vec![
        pb::Offering { required_room_count: 2, ..offering("multi", 1) },
        offering("single", 1),
    ];

    let problem = convert(&input, &scope(&["multi", "single"])).expect("valid input");
    let outcome = solve(&problem, SEED, budget(), &NeverHalt);
    let output = build_output(&problem, &outcome, 0);
    assert_eq!(output.sessions.len(), 2);

    let multi = output
        .sessions
        .iter()
        .find(|s| s.offering_id == "multi")
        .unwrap();
    assert_eq!(
        multi.room_ids.len(),
        2,
        "a multi-Room placement's full set, room_id included, must be written"
    );
    assert!(multi.room_ids.contains(&multi.room_id));

    let single = output
        .sessions
        .iter()
        .find(|s| s.offering_id == "single")
        .unwrap();
    assert!(
        single.room_ids.is_empty(),
        "an ordinary single-Room Session must not carry a redundant one-element echo"
    );
}

#[test]
fn required_room_count_of_zero_or_one_is_unaffected() {
    let mut input = base_input();
    input.offerings = vec![
        offering("zero", 1),
        pb::Offering { required_room_count: 1, ..offering("one", 1) },
    ];

    let problem = convert(&input, &scope(&["zero", "one"])).expect("valid input");
    for o in &problem.offerings {
        assert!(
            !o.eligible_rooms.is_empty(),
            "{}: eligible_rooms must be populated exactly as before required_room_count existed",
            o.id
        );
        assert!(
            o.eligible_room_combinations.is_empty(),
            "{}: combinations are only ever computed for required_room_count > 1",
            o.id
        );
    }
}
