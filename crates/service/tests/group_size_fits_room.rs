//! `GroupSizeFitsRoom` at the wire boundary: `Group.size` and `Room.capacity`
//! are already threaded from the wire into core — this only checks that the
//! constraint config actually switches the cross-check on, and that a solved
//! instance surfaces the violation `evaluate_hard` reports.

use calendry_solver::convert::convert;
use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, room, scope};

fn budget() -> Budget {
    Budget { max_wall_millis: 0, max_moves: 5_000 }
}

fn group_size_fits_room() -> pb::ConstraintConfig {
    enabled("c-size", pb::constraint_config::Params::GroupSizeFitsRoom(pb::GroupSizeFitsRoom {}))
}

#[test]
fn a_group_larger_than_its_room_is_reported_once_solved() {
    let mut input = base_input();
    input.rooms = vec![pb::Room { capacity: 20, ..room(0) }];
    input.groups = vec![pb::Group { size: 40, ..common::group("g1") }];
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(group_size_fits_room());

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.constraints.group_size_fits_room.len(), 1);

    let outcome = solve(&problem, 0xC0FFEE, budget(), &NeverHalt);
    let violations = evaluate_hard(&problem, &outcome.solution);
    assert!(
        violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::GroupSizeFitsRoom),
        "40-person Group in a 20-seat room: {violations:?}"
    );
}

#[test]
fn not_configured_means_not_parsed() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    // `group_size_fits_room()` never pushed.

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert!(problem.constraints.group_size_fits_room.is_empty());
}
