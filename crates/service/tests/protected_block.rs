//! `ProtectedBlock` at the wire boundary: `pb::BlockedWindow` maps onto
//! core's `Unavailability`, and the search actually avoids the reserved slot
//! end to end.

use calendry_solver::convert::convert;
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, one_slot_grid, scope};

fn budget() -> Budget {
    Budget { max_wall_millis: 0, max_moves: 2_000 }
}

fn protected(windows: Vec<pb::BlockedWindow>) -> pb::ConstraintConfig {
    enabled(
        "c-protected",
        pb::constraint_config::Params::ProtectedBlock(pb::ProtectedBlock { windows }),
    )
}

#[test]
fn the_window_reaches_the_offerings_mask() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(protected(vec![pb::BlockedWindow {
        days: vec![],
        blocks: vec![0],
        weeks: vec![],
    }]));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.constraints.protected_block.len(), 1);
    assert_eq!(problem.constraints.protected_block[0].windows[0].blocks, vec![0]);
}

#[test]
fn the_search_avoids_a_reserved_slot_with_no_other_room() {
    // One slot, one room: block 0 reserved (empty days/weeks = every day,
    // every week) makes the Offering genuinely unplaceable.
    let mut input = base_input();
    one_slot_grid(&mut input);
    input.rooms = vec![common::room(0)];
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(protected(vec![pb::BlockedWindow {
        days: vec![],
        blocks: vec![],
        weeks: vec![],
    }]));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    let outcome = solve(&problem, 0xC0FFEE, budget(), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 1, "the only slot is reserved");
}
