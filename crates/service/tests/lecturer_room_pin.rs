//! `Person.allowed_room_ids` across the wire (Calendry #124): a fixed Room for
//! a Person.
//!
//! The rule itself is covered in `crates/core/tests/lecturer_room_pin.rs`.
//! What is checked here is the boundary, which has one refusal and three
//! deliberate NON-refusals — and each of the three is a place where the
//! obvious reading of an existing precedent would be wrong:
//!
//! * an unknown Room id is REFUSED, because dropping it shrinks a whitelist
//!   and shrinking it to empty means "any Room";
//! * a pin naming a VIRTUAL Room is HONOURED, which is the opposite of
//!   `FootprintOnVirtualRoom` — a footprint tag on a virtual Room could only
//!   ever be inert, while "this person only teaches online" is a real
//!   statement;
//! * a pin combined with a genuine lecturer POOL is ACCEPTED — the precedent
//!   `LecturerVeto` now follows too (Calendry #131), having been refused in
//!   that combination while its mask was still precomputed per Offering;
//! * an empty list is inert, not a lockout.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_core::ids::{PersonIdx, RoomIdx};
use calendry_solver_proto::v1 as pb;
use tonic::Code;

mod common;
use common::{base_input, enabled, offering, person, room, scope};

fn pin_rule(kinds: &[&str]) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        applies_to_kinds: kinds.iter().map(|k| (*k).to_string()).collect(),
        ..enabled(
            "c-room-pin",
            pb::constraint_config::Params::LecturerRoomPin(pb::LecturerRoomPin {}),
        )
    }
}

fn pinned(id: &str, rooms: &[&str]) -> pb::Person {
    pb::Person { allowed_room_ids: rooms.iter().map(|r| (*r).to_string()).collect(), ..person(id) }
}

/// Whether the converted problem bars `person` from `room` — the same
/// predicate the search and the report both use.
fn barred(problem: &calendry_solver_core::Problem, person: u32, room: u32) -> bool {
    problem.room_pin_blocks(std::iter::once(PersonIdx(person)), std::iter::once(RoomIdx(room)))
}

// ---------------------------------------------------------------------------
// The one refusal
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_room_in_a_pin_is_invalid_argument() {
    // A whitelist cannot afford a silent drop: remove the last id and the pin
    // becomes "any Room", so a restriction that was real in the database
    // arrives as no restriction at all with the run reporting nothing wrong.
    // `group_ids` on the same message already refuses for the mirror reason.
    let mut input = base_input();
    input.persons = vec![pinned("p1", &["ghost"])];
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(pin_rule(&[]));

    let error = convert(&input, &scope(&["o1"])).expect_err("this input must be refused");

    assert!(
        matches!(&error, ConvertError::UnknownRoom { context, room }
            if context.contains("p1") && room == "ghost"),
        "the context must name the Person so the app can find it: {error}",
    );
    assert_eq!(tonic::Status::from(error).code(), Code::InvalidArgument);
}

// ---------------------------------------------------------------------------
// The three non-refusals
// ---------------------------------------------------------------------------

#[test]
fn a_pin_naming_a_virtual_room_is_accepted() {
    // NOT `FootprintOnVirtualRoom`, and the distinction is the whole point. A
    // footprint tag on a virtual Room is refused because a virtual Room's
    // occupancy row is never consulted, so the tag could only ever be inert. A
    // pin is not inert on a virtual Room: the Room's identity is read by
    // `allow_online`, by `MinimizeOnline`, and by the placement itself.
    let mut input = base_input();
    input.rooms = vec![room(0), pb::Room { is_virtual: true, ..room(1) }];
    input.persons = vec![pinned("p1", &["r1"])];
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(pin_rule(&[]));

    let problem = convert(&input, &scope(&["o1"])).expect("a pin on a virtual Room is real");

    assert!(barred(&problem, 0, 0), "the physical Room is barred");
    assert!(!barred(&problem, 0, 1), "and the virtual one is the pin, honoured exactly");
}

#[test]
fn a_pin_combined_with_a_lecturer_pool_is_accepted() {
    // NEGATIVE-SPACE TEST. `LecturerVeto` plus a genuine pool WAS refused
    // while its mask was precomputed from `Offering::lecturers`, which a pool
    // leaves empty. This rule asked the question against the CHOSEN lecturers
    // from the start, so a pool is the case it serves rather than the case it
    // breaks — and this test exists to pin that the refusal was never copied
    // along with the shape. (Since Calendry #131 the veto asks the same way;
    // see `lecturer_veto_pool.rs` next door.)
    let mut input = base_input();
    input.persons = vec![pinned("p1", &["r0"]), pinned("p2", &["r1"])];
    input.offerings = vec![pb::Offering {
        candidate_lecturer_ids: vec!["p1".into(), "p2".into()],
        required_lecturer_count: 1,
        ..offering("o1", 1)
    }];
    input.constraints.push(pin_rule(&[]));

    let problem = convert(&input, &scope(&["o1"]))
        .expect("a room pin and a genuine lecturer pool must coexist");

    assert!(!barred(&problem, 0, 0), "p1 keeps R0");
    assert!(!barred(&problem, 1, 1), "p2 keeps R1");
    assert!(barred(&problem, 0, 1), "and each is still barred from the other's");
}

#[test]
fn an_empty_allowed_room_ids_means_any_room_on_the_wire() {
    // #123's sharpest trap, at the boundary: empty means "any Room", never
    // "no Room". A conversion that inverted the whitelist without this case
    // would make every unconfigured Person unplaceable everywhere — which is
    // every Person on every tenant that has not adopted the field.
    let mut input = base_input();
    input.persons = vec![person("p1")];
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(pin_rule(&[]));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    for r in 0..2 {
        assert!(!barred(&problem, 0, r), "R{r} must stay available with no pin stated");
    }
}

// ---------------------------------------------------------------------------
// Enablement
// ---------------------------------------------------------------------------

#[test]
fn a_pin_is_inert_until_lecturer_room_pin_is_enabled() {
    // The `LecturerVeto`/`GroupVeto` split: the values ride on the Person, the
    // switch is tenant policy. The values still convert — they are Person data
    // either way — so the inertness is in `Enforce`, which is where a tenant
    // can turn the rule on without re-sending every Person.
    let mut input = base_input();
    input.persons = vec![pinned("p1", &["r1"])];
    input.offerings = vec![offering("o1", 1)];
    input
        .constraints
        .push(pb::ConstraintConfig { enabled: false, ..pin_rule(&[]) });

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    assert!(
        !problem.offerings[0].enforce.lecturer_room_pin,
        "a disabled instance must not enforce"
    );
}

#[test]
fn a_pin_is_scoped_by_applies_to_kinds() {
    let build = |kinds: &[&str]| {
        let mut input = base_input();
        input.persons = vec![pinned("p1", &["r1"])];
        input.offerings = vec![offering("o1", 1)];
        input.constraints.push(pin_rule(kinds));
        convert(&input, &scope(&["o1"])).expect("valid input")
    };

    // `common::offering` uses the fixture's own KIND, which the first case
    // names and the second deliberately does not.
    let kind = build(&[]).offerings[0].kind.clone();
    assert!(build(&[&kind]).offerings[0].enforce.lecturer_room_pin, "covered");
    assert!(
        !build(&["staff_meeting"]).offerings[0]
            .enforce
            .lecturer_room_pin,
        "a rule scoped to another kind must not reach this Offering"
    );
}
