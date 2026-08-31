//! `TravelTimeBetweenRooms` at the wire boundary: `CompactnessScope` parses
//! the same way `MaxConsecutiveBlocks` does, and weight IS validated —
//! SOFT, like `RoomTurnaroundBuffer`/`Daybreak`.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn travel(weight: f64, scope_axes: Vec<i32>, min_minutes: u32) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-travel",
            pb::constraint_config::Params::TravelTimeBetweenRooms(pb::TravelTimeBetweenRooms {
                scope: scope_axes,
                min_minutes_between_sites: min_minutes,
            }),
        )
    }
}

#[test]
fn an_empty_scope_means_both_axes() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(travel(5.0, vec![], 15));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.constraints.travel_time_between_rooms.len(), 1);
    assert!(problem.constraints.travel_time_between_rooms[0].group);
    assert!(problem.constraints.travel_time_between_rooms[0].person);
    assert_eq!(problem.constraints.travel_time_between_rooms[0].min_minutes_between_sites, 15);
}

#[test]
fn a_group_only_scope_leaves_the_person_axis_unset() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input
        .constraints
        .push(travel(5.0, vec![pb::CompactnessScope::Group as i32], 15));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert!(problem.constraints.travel_time_between_rooms[0].group);
    assert!(!problem.constraints.travel_time_between_rooms[0].person);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(travel(-1.0, vec![], 15));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(e, ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0));
}

#[test]
fn a_zero_weight_is_accepted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(travel(0.0, vec![], 15));

    let problem = convert(&input, &scope(&["o1"])).expect("zero is a valid weight");
    assert_eq!(problem.constraints.travel_time_between_rooms[0].weight, 0.0);
}

#[test]
fn room_location_reaches_core() {
    let mut input = base_input();
    input.rooms = vec![
        pb::Room { location: "Building A".into(), ..common::room(0) },
        pb::Room { location: "Building B".into(), ..common::room(1) },
    ];
    input.offerings = vec![offering("o1", 1)];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.rooms[0].location, "Building A");
    assert_eq!(problem.rooms[1].location, "Building B");
}
