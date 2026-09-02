//! `Room.footprint_tags` across the wire (Calendry #122): movable walls, where
//! several Room identities describe one physical space.
//!
//! The core behaviour is covered in `crates/core/tests/room_footprint.rs`. What
//! is checked here is the boundary: that a tag arrives as a resolved sibling
//! set rather than as a string nobody reads, that a tag naming nothing is
//! inert, and that the one configuration whose effect would be SILENT — a tag
//! on a virtual Room — is refused instead.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_core::ids::RoomIdx;
use calendry_solver_proto::v1 as pb;
use tonic::Code;

mod common;
use common::{base_input, footprint_room, offering, room, scope};

#[test]
fn a_shared_tag_resolves_to_a_symmetric_sibling_set() {
    // Three sub-rooms and the combined room, one tag. Every member must name
    // the other three and never itself — and it must be the SAME statement
    // read from any of them, since a directed model is what would let the two
    // directions disagree.
    let mut input = base_input();
    input.rooms = (0..4).map(|i| footprint_room(i, &["audimax"])).collect();
    input.offerings = vec![offering("o1", 1)];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    for i in 0..4u32 {
        let siblings: Vec<u32> = problem
            .footprint_siblings(RoomIdx(i))
            .iter()
            .map(|r| r.get() as u32)
            .collect();
        let expected: Vec<u32> = (0..4).filter(|&o| o != i).collect();
        assert_eq!(siblings, expected, "room {i} must name the other three, and not itself");
    }
}

#[test]
fn a_room_with_no_tag_shares_nothing() {
    // The control. An ordinary Room alongside a footprint group must stay
    // completely outside it — this field costs nothing for the tenants that
    // have no folding walls, which is nearly all of them.
    let mut input = base_input();
    input.rooms = vec![
        footprint_room(0, &["audimax"]),
        footprint_room(1, &["audimax"]),
        room(2),
    ];
    input.offerings = vec![offering("o1", 1)];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    assert_eq!(problem.footprint_siblings(RoomIdx(2)), &[]);
    assert_eq!(problem.footprint_siblings(RoomIdx(0)), &[RoomIdx(1)]);
}

#[test]
fn a_tag_no_other_room_carries_is_inert_rather_than_refused() {
    // A wall configuration half entered, which the app's "warn and allow"
    // editing UX produces routinely. It must resolve to nothing rather than
    // failing the whole run — there is no fault here, only an incomplete
    // group, and the solver tolerates infeasible and incomplete input.
    let mut input = base_input();
    input.rooms = vec![footprint_room(0, &["half-configured"]), room(1)];
    input.offerings = vec![offering("o1", 1)];

    let problem = convert(&input, &scope(&["o1"])).expect("an unshared tag is not a fault");

    assert_eq!(problem.footprint_siblings(RoomIdx(0)), &[]);
}

#[test]
fn a_footprint_on_a_virtual_room_is_refused() {
    // THE ONE REFUSAL. A virtual Room's occupancy row is never consulted
    // (ADR-0022), so a tag on it can only ever be inert — and inert is the
    // worst available outcome: the caller believes they declared a hard
    // exclusivity, and every run afterwards reports zero violations while the
    // space is double-booked. Refusing names the fault where it can be fixed.
    let mut input = base_input();
    input.rooms = vec![
        footprint_room(0, &["audimax"]),
        pb::Room { is_virtual: true, ..footprint_room(1, &["audimax"]) },
    ];
    input.offerings = vec![offering("o1", 1)];

    let error = convert(&input, &scope(&["o1"])).expect_err("this input must be refused");

    assert!(
        matches!(&error, ConvertError::FootprintOnVirtualRoom { room, .. } if room == "r1"),
        "unexpected error: {error}",
    );
    assert_eq!(
        tonic::Status::from(error).code(),
        Code::InvalidArgument,
        "bad input the caller can fix, not an unbuilt feature",
    );
}

#[test]
fn a_virtual_room_with_no_tag_is_untouched() {
    // The refusal is scoped to the combination, not to virtual Rooms in
    // general — online delivery must keep working exactly as before.
    let mut input = base_input();
    input.rooms = vec![
        footprint_room(0, &["audimax"]),
        pb::Room { is_virtual: true, ..room(1) },
    ];
    input.offerings = vec![offering("o1", 1)];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    assert_eq!(problem.footprint_siblings(RoomIdx(1)), &[]);
}
