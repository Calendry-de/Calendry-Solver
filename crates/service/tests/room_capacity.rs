//! `Room.capacity == 0` means UNBOUNDED, not "fits nobody" (issue #62). The
//! app's column defaults to 0, so a Room saved with nothing recorded must
//! stay eligible for any Offering rather than becoming silently ineligible
//! for every one that asks for a minimum.

use calendry_solver_core::ids::{OfferingIdx, RoomIdx};
use calendry_solver_proto::v1 as pb;

use calendry_solver::convert::convert;

mod common;
use common::{base_input, offering, room, scope};

#[test]
fn a_zero_capacity_room_is_eligible_for_any_min_capacity() {
    let mut input = base_input();
    input.rooms = vec![pb::Room { capacity: 0, ..room(0) }];
    input.offerings = vec![pb::Offering { min_capacity: 500, ..offering("o1", 1) }];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(
        problem.offerings[OfferingIdx(0).get()].eligible_rooms,
        vec![RoomIdx(0)],
        "an unmeasured (0) capacity must not exclude the Room"
    );
}

#[test]
fn a_positive_but_insufficient_capacity_still_excludes_the_room() {
    // The fix is specifically for 0, not a general "never refuse a Room" —
    // an actually-measured, too-small Room must stay ineligible.
    let mut input = base_input();
    input.rooms = vec![pb::Room { capacity: 10, ..room(0) }];
    input.offerings = vec![pb::Offering { min_capacity: 500, ..offering("o1", 1) }];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert!(
        problem.offerings[OfferingIdx(0).get()]
            .eligible_rooms
            .is_empty(),
        "10 measured seats really is too few for 500"
    );
}

#[test]
fn one_unbounded_room_in_a_multi_room_combination_makes_the_whole_combination_eligible() {
    // Room 0 is unbounded (0), Room 1 has only 10 seats. Summing raw
    // capacities (0 + 10 = 10) would wrongly refuse a 500-seat requirement;
    // the unbounded Room must carry the whole combination instead.
    let mut input = base_input();
    input.rooms = vec![
        pb::Room { capacity: 0, ..room(0) },
        pb::Room { capacity: 10, ..room(1) },
    ];
    input.offerings =
        vec![pb::Offering { required_room_count: 2, min_capacity: 500, ..offering("o1", 1) }];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    let combos = &problem.offerings[OfferingIdx(0).get()].eligible_room_combinations;
    assert_eq!(combos.len(), 1, "the only possible pair must be accepted as unbounded");
}
