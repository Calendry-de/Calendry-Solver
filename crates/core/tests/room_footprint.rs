//! `Room.footprints`: several Room identities over one physical space
//! (Calendry #122).
//!
//! Rooms 1.0, 1.1 and 1.2 sit behind folding partitions. Open every wall and
//! it is the Audimax; close them and it is three independent rooms. Booking
//! 1.0 must make 1.1, 1.2 AND the Audimax unbookable for that slot, and
//! booking the Audimax must make all three unbookable — one footprint, four
//! Room rows.
//!
//! Nothing in the model could say that before this. `no_double_booking_room`
//! is one Room against ITSELF across time, its exclusivity read from
//! `Room::is_exclusive` (ADR-0022); there was no way to state that booking
//! Room A also occupies Room B. A shared open-vocabulary tag is the smallest
//! thing that can, and it is symmetric by construction, so the two directions
//! cannot drift apart — which is what the mirrored pair below pins.
//!
//! It is a FILTER, not a price: two Sessions in one physical space at one hour
//! is not a preference to weigh. So the assertions are mostly `is_free`, the
//! question the search itself asks, rather than a cost.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::ids::{OfferingIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ProblemSpec, RelationKind, RelationSpec, Room};
use calendry_solver_core::solution::{Occupant, Placement, SearchState};
use calendry_solver_core::testing::{self, footprint_rooms};
use calendry_solver_core::{Problem, Solution};

mod common;
use common::solve_with_move_budget as run;

/// `two_day_grid` is one block on Monday and one on Saturday.
const MONDAY: SlotIdx = SlotIdx(0);
const SATURDAY: SlotIdx = SlotIdx(1);

/// The Audimax fixture: four Rooms over one footprint, plus one ordinary Room
/// that shares nothing, and two single-Session Offerings each eligible for
/// every Room.
///
/// Naming, for the assertions below: `R0`/`R1`/`R2` are the sub-rooms, `R3` is
/// the combined room, `R4` is unrelated.
fn audimax(offerings: usize) -> Problem {
    let mut rooms = footprint_rooms("audimax", &["R0", "R1", "R2", "R3"]);
    rooms.push(testing::room("R4"));
    let eligible: Vec<u32> = (0..rooms.len() as u32).collect();

    testing::assemble(ProblemSpec {
        rooms,
        offerings: (0..offerings)
            .map(|i| testing::offering(&format!("S{i}"), 1, &eligible))
            .collect(),
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::two_day_grid())
    })
}

/// Mark one Session into `room` at `slot`, then report which Rooms a second
/// Session could still take at that same slot.
fn free_rooms_after_booking(problem: &Problem, room: u32, slot: SlotIdx) -> Vec<u32> {
    let mut state = SearchState::from_fixed(problem);
    let span = problem.slots.span(slot, 1).expect("slot in grid");

    let first = Occupant::of_offering(&problem.offerings[0]).with_room(RoomIdx(room));
    state.mark(problem, &first, &span);

    (0..problem.rooms.len() as u32)
        .filter(|&r| {
            let second = Occupant::of_offering(&problem.offerings[1]).with_room(RoomIdx(r));
            state.is_free(problem, &second, &span)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The mirrored pair: the whole point is that it works in both directions
// ---------------------------------------------------------------------------

#[test]
fn booking_a_sub_room_occupies_its_siblings_and_the_combined_room() {
    let problem = audimax(2);

    assert_eq!(
        free_rooms_after_booking(&problem, 0, MONDAY),
        vec![4],
        "1.0 takes 1.1, 1.2 and the Audimax with it; only the unrelated Room is left",
    );
}

#[test]
fn booking_the_combined_room_occupies_every_sub_room() {
    // THE MIRROR. A directed "A also books B" model would need this stated
    // twice and could be built with one direction missing; a shared tag cannot
    // be. This test is what would catch it if the closure were ever rebuilt as
    // a reference.
    let problem = audimax(2);

    assert_eq!(
        free_rooms_after_booking(&problem, 3, MONDAY),
        vec![4],
        "opening the walls takes all three sub-rooms",
    );
}

#[test]
fn a_room_sharing_no_footprint_is_untouched() {
    // The control, and the reason the two assertions above are not vacuous:
    // booking the unrelated Room leaves every footprint member free.
    let problem = audimax(2);

    assert_eq!(free_rooms_after_booking(&problem, 4, MONDAY), vec![0, 1, 2, 3]);
}

#[test]
fn the_occupancy_is_per_slot_not_per_day() {
    // A footprint is a claim about a space at a time. Monday's booking must
    // say nothing about Saturday.
    let problem = audimax(2);
    let mut state = SearchState::from_fixed(&problem);
    let monday = problem.slots.span(MONDAY, 1).expect("in grid");
    let saturday = problem.slots.span(SATURDAY, 1).expect("in grid");

    let first = Occupant::of_offering(&problem.offerings[0]).with_room(RoomIdx(0));
    state.mark(&problem, &first, &monday);

    let second = Occupant::of_offering(&problem.offerings[1]).with_room(RoomIdx(3));
    assert!(!state.is_free(&problem, &second, &monday));
    assert!(state.is_free(&problem, &second, &saturday));
}

// ---------------------------------------------------------------------------
// Bookkeeping: what is marked must come back
// ---------------------------------------------------------------------------

#[test]
fn unmark_releases_every_bit_mark_claimed() {
    // `mark`/`unmark` are balanced today because the footprint is expanded on
    // the query side and neither touches a sibling row. This pins that: the
    // moment either one starts writing sibling bits, they have to write the
    // same ones, and a leak is silent and cumulative — one Room lost from the
    // instance per rejected move, and LNS rejects most of what it tries.
    // Nothing would report it; the run would simply place less and less.
    let problem = audimax(2);
    let mut state = SearchState::from_fixed(&problem);
    let span = problem.slots.span(MONDAY, 1).expect("in grid");
    let first = Occupant::of_offering(&problem.offerings[0]).with_room(RoomIdx(0));

    state.mark(&problem, &first, &span);
    state.unmark(&problem, &first, &span);

    for r in 0..5 {
        let second = Occupant::of_offering(&problem.offerings[1]).with_room(RoomIdx(r));
        assert!(state.is_free(&problem, &second, &span), "room {r} must be free again");
    }
}

// ---------------------------------------------------------------------------
// Degenerate configurations, which a tenant mid-edit will produce
// ---------------------------------------------------------------------------

#[test]
fn a_tag_only_one_room_carries_is_inert() {
    // A wall configuration part-way through being entered. It must cost
    // nothing rather than blocking anything, and in particular a Room must not
    // become its own sibling and block itself.
    let mut rooms = footprint_rooms("solo", &["R0"]);
    rooms.push(testing::room("R1"));
    let problem = testing::assemble(ProblemSpec {
        rooms,
        offerings: (0..2)
            .map(|i| testing::offering(&format!("S{i}"), 1, &[0, 1]))
            .collect(),
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::two_day_grid())
    });

    assert!(problem.footprint_siblings(RoomIdx(0)).is_empty());
    assert_eq!(free_rooms_after_booking(&problem, 0, MONDAY), vec![1]);
}

#[test]
fn a_room_may_sit_in_two_footprints_at_once() {
    // The ticket's open question, answered by the tag model for free: a wall
    // shared between two combination options. `mid` divides A from B, so
    // booking `mid` blocks both, while A and B never block each other.
    //
    // THE TRANSITIVITY TRAP, and the reason `Occupancy` expands a footprint on
    // the query side rather than on `mark`. Marking the siblings instead is
    // the obvious implementation and passes every other test in this file: it
    // would set `mid`'s bit when A is booked, and `B` — which shares a wall
    // with `mid` — would then read that bit and refuse a slot it is entitled
    // to. Overlap is symmetric but NOT transitive.
    let rooms = vec![
        Room { footprints: vec!["a".into()], ..testing::room("A") },
        Room { footprints: vec!["a".into(), "b".into()], ..testing::room("mid") },
        Room { footprints: vec!["b".into()], ..testing::room("B") },
    ];
    let problem = testing::assemble(ProblemSpec {
        rooms,
        offerings: (0..2)
            .map(|i| testing::offering(&format!("S{i}"), 1, &[0, 1, 2]))
            .collect(),
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::two_day_grid())
    });

    assert_eq!(free_rooms_after_booking(&problem, 1, MONDAY), Vec::<u32>::new(), "mid blocks both");
    assert_eq!(free_rooms_after_booking(&problem, 0, MONDAY), vec![2], "A leaves B alone");
    assert_eq!(free_rooms_after_booking(&problem, 2, MONDAY), vec![0], "and B leaves A alone");
}

#[test]
fn a_virtual_rooms_footprint_is_dropped_rather_than_enforced() {
    // Core softens what the wire refuses outright: online delivery is a Room
    // so that room-assignment logic stays uniform, its occupancy row is never
    // consulted (ADR-0022), and letting a tag on it mark a physical Room's row
    // would cap all online teaching behind a wall it does not stand in.
    let rooms = vec![
        Room { footprints: vec!["hall".into()], ..testing::room("R0") },
        Room {
            is_virtual: true,
            footprints: vec!["hall".into()],
            ..testing::room_with("ONLINE", 1, true)
        },
    ];
    let problem = testing::assemble(ProblemSpec {
        rooms,
        offerings: (0..2)
            .map(|i| testing::offering(&format!("S{i}"), 1, &[0, 1]))
            .collect(),
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::two_day_grid())
    });

    assert!(problem.footprint_siblings(RoomIdx(0)).is_empty());
    assert_eq!(free_rooms_after_booking(&problem, 0, MONDAY), vec![1]);
}

// ---------------------------------------------------------------------------
// End to end, and the report
// ---------------------------------------------------------------------------

#[test]
fn meet_together_members_still_share_a_room_that_has_a_footprint() {
    // The one legitimate way two Sessions occupy one exclusive Room at one
    // time (issue #55). Nothing about a folding wall changes that: the members
    // hold ONE Room, and the footprint check must not turn their own shared
    // cell into a clash with themselves.
    let a = testing::with_min_capacity(testing::offering("a", 1, &[0, 1]), 10);
    let b = testing::with_min_capacity(testing::offering("b", 1, &[0, 1]), 10);
    let mut spec = ProblemSpec {
        rooms: footprint_rooms("audimax", &["R0", "R1"])
            .into_iter()
            .map(|r| Room { capacity: 100, ..r })
            .collect(),
        offerings: vec![a, b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::MeetTogether,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..testing::fixture(testing::two_day_grid(), testing::structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).expect("no group cycle");

    let outcome = run(&problem);
    let a = outcome.solution.get(PlacementIdx(0)).expect("placed");
    let b = outcome.solution.get(PlacementIdx(1)).expect("placed");

    assert_eq!((a.start, a.room), (b.start, b.room), "one meeting, one Room, one slot");
    assert!(evaluate_hard(&problem, &outcome.solution).is_empty());
}

#[test]
fn the_search_separates_two_sessions_over_one_footprint() {
    // Two Sessions, three Rooms all over one footprint, two slots. Whatever
    // Rooms the run picks, it cannot put both Sessions in the same slot.
    let problem = testing::assemble(ProblemSpec {
        rooms: footprint_rooms("audimax", &["R0", "R1", "R2"]),
        offerings: (0..2)
            .map(|i| testing::offering(&format!("S{i}"), 1, &[0, 1, 2]))
            .collect(),
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::two_day_grid())
    });

    let outcome = run(&problem);
    let a = outcome.solution.get(PlacementIdx(0)).expect("placed");
    let b = outcome.solution.get(PlacementIdx(1)).expect("placed");

    assert_ne!(a.start, b.start, "one space cannot hold two Sessions at one hour");
    assert!(evaluate_hard(&problem, &outcome.solution).is_empty());
}

#[test]
fn a_clash_across_a_wall_is_reported_as_a_room_double_booking() {
    // The independent checker (ADR-0014). The search can never produce this
    // pair, but a caller's snapshot can — two locked Sessions either side of a
    // folding wall — and this is what tells the timetabler about it.
    let problem = audimax(2);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(MONDAY, RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(MONDAY, RoomIdx(3))));

    let clashes: Vec<_> = evaluate_hard(&problem, &solution)
        .into_iter()
        .filter(|v| v.constraint_type == ConstraintType::RoomDoubleBooking)
        .collect();

    assert_eq!(clashes.len(), 1, "one clash, reported once: {clashes:?}");
    let detail = &clashes[0].detail;
    assert!(
        detail.contains("R0") && detail.contains("R3") && detail.contains("footprint"),
        "the report must name BOTH Rooms and why they clash: {detail}",
    );
}

#[test]
fn two_sessions_in_the_same_room_still_report_the_plain_message() {
    // The `else if` ordering: an identical Room is the stronger statement, and
    // a pair sharing both a Room and a footprint must be named once, not twice.
    let problem = audimax(2);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(MONDAY, RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(MONDAY, RoomIdx(0))));

    let clashes: Vec<_> = evaluate_hard(&problem, &solution)
        .into_iter()
        .filter(|v| v.constraint_type == ConstraintType::RoomDoubleBooking)
        .collect();

    assert_eq!(clashes.len(), 1, "one clash, one violation: {clashes:?}");
    assert!(
        clashes[0].detail.contains("room 'R0' hosts"),
        "same Room reads as the plain double booking: {}",
        clashes[0].detail,
    );
}
