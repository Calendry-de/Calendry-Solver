//! Why `CanShareRoom` is not built, and must not be built on `MeetTogether`'s
//! machinery (issue #55).
//!
//! CLAUDE.md has long listed `CanShareRoom` as the last unbuilt
//! `OfferingRelation` type, needing only "its own answer to what sharing means
//! without `SameTime`/`SameDays` holding the pair together", and adds that
//! "none is mechanism work". The first half is right. The second is wrong, and
//! this file is the reason made executable — the same pattern
//! `mid_week_absence.rs` uses, because the cost of getting it wrong is a hard
//! structural rule quietly acquiring a hole.
//!
//! **What the name means.** `UniTime`'s "Meet Together" is the package
//! "Can Share Room" + "Same Room" + "Same Time" + "Same Days", so
//! `CanShareRoom` is the PERMISSION primitive: these Offerings *may*
//! co-occupy one Room where they happen to overlap. (The "they should land in
//! the same Room" reading is `UniTime`'s separate "Same Room" — a different
//! unbuilt type. That two readings share one name is itself a reason not to
//! build from the name.)
//!
//! **A permission is an exemption, not a constraint.** Every kind on this
//! mechanism either filters or is hard-and-priced. A permission only ever
//! widens the feasible set, so it can never be violated: no evaluator, no
//! `ConstraintType`, no objective term, nothing to report. It is a hole in
//! `RoomDoubleBooking` — and ADR-0014 then requires a matching exemption in
//! the independent `check_pair`, or the search would refuse placements it
//! declines to report. `MeetTogether` paid that price to buy a rule; this
//! would pay it to buy a relaxation.
//!
//! **And the anchor cannot carry it.** `MeetTogether`'s exemption is safe
//! because `Occupancy` never needs to know *who* holds an occupied cell: the
//! anchor pins an exact `(start, end, room)` per `(relation, week)`, so any
//! bit inside that span provably belongs to a fellow member. The consequence
//! — tested below — is that the anchor is CHAIN-TRANSITIVE. That is exactly
//! right for `MeetTogether`, because "is the same physical meeting" is an
//! equivalence relation. It is exactly wrong for a permission, which is only
//! symmetric: if A may share with B and B with C, A may not thereby share
//! with C. Building `CanShareRoom` on this anchor reintroduces ADR-0022's
//! transitivity bug, and doing it properly needs per-cell occupant identity
//! that `Occupancy` deliberately does not carry. That is mechanism work.
//!
//! **Capacity relief is a Room axis, not a relation.** `Room::is_exclusive()`
//! is `!is_virtual`, so there is no non-exclusive physical Room to express
//! "this hall seats two classes at once" — the field ADR-0022 already
//! identified as missing. The last test pins that gap.

use calendry_solver_core::Placement;
use calendry_solver_core::ids::{OfferingIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{Problem, ProblemSpec, RelationKind, RelationSpec, Room};
use calendry_solver_core::solution::SearchState;
use calendry_solver_core::testing::{
    fixture, footprint_rooms, grid, offering, room, structural_room_only, with_min_capacity,
};

mod common;

fn room_with_capacity(id: &str, capacity: u32) -> Room {
    Room { capacity, ..room(id) }
}

/// Three Offerings and TWO overlapping relations — `{A, B}` and `{B, C}`.
/// `A` and `C` are in no relation together, which is the whole point.
fn overlapping_relations(rooms: Vec<Room>, min_capacity: u32) -> Problem {
    let eligible: Vec<u32> = (0..rooms.len() as u32).collect();
    let mut spec = ProblemSpec {
        rooms,
        offerings: vec![
            with_min_capacity(offering("a", 1, &eligible), min_capacity),
            with_min_capacity(offering("b", 1, &eligible), min_capacity),
            with_min_capacity(offering("c", 1, &eligible), min_capacity),
        ],
        relations: vec![
            RelationSpec {
                id: "ab".to_string(),
                kind: RelationKind::MeetTogether,
                members: vec![OfferingIdx(0), OfferingIdx(1)],
            },
            RelationSpec {
                id: "bc".to_string(),
                kind: RelationKind::MeetTogether,
                members: vec![OfferingIdx(1), OfferingIdx(2)],
            },
        ],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    Problem::build(spec).unwrap()
}

// ---------------------------------------------------------------------------
// The anchor is chain-transitive — right for sameness, wrong for permission
// ---------------------------------------------------------------------------

#[test]
fn meet_together_is_an_equivalence_and_that_is_why_the_anchor_is_safe() {
    // `{A, B}` and `{B, C}`: all three end up in ONE cell of ONE Room, and
    // the capacity sum prices all three against it. Previously untested —
    // the existing suite covers only two DISJOINT relations.
    //
    // This is correct behaviour. "Is the same physical meeting" is an
    // equivalence relation, so its transitive closure is the right answer,
    // and that is what lets a `(relation, week)` anchor carry it without
    // tracking who holds which cell.
    let problem = overlapping_relations(vec![room_with_capacity("r0", 100)], 10);
    let mut state = SearchState::from_fixed(&problem);
    let cell = Placement::single(SlotIdx(0), RoomIdx(0));

    assert!(state.place(&problem, PlacementIdx(0), cell), "A takes the cell");
    assert!(state.place(&problem, PlacementIdx(1), cell), "B joins through {{A, B}}");
    assert!(state.place(&problem, PlacementIdx(2), cell), "C joins through {{B, C}}");
}

#[test]
fn permission_would_not_be_transitive_so_the_anchor_cannot_carry_it() {
    // THE MOST IMPORTANT ASSERTION IN THIS FILE.
    //
    // A and C share NO relation, yet C is admitted into the cell A opened —
    // because B's mark anchored both relations at the same span, and the
    // per-slot check trusts the anchor rather than re-deriving whose sibling
    // each occupied bit is.
    //
    // Read as "is the same meeting", that is correct and desirable. Read as
    // "may share a room with", it is a bug: permission is symmetric but NOT
    // transitive, so A must not inherit a permission it never had. A
    // `CanShareRoom` reusing this machinery would ship exactly ADR-0022's
    // transitivity defect — the one that ADR forbids by expanding the
    // footprint on the QUERY side and never in `mark`.
    let problem = overlapping_relations(vec![room_with_capacity("r0", 100)], 10);
    let mut state = SearchState::from_fixed(&problem);
    let cell = Placement::single(SlotIdx(0), RoomIdx(0));

    assert!(state.place(&problem, PlacementIdx(0), cell));
    assert!(state.place(&problem, PlacementIdx(1), cell));

    assert!(
        state.can_place(&problem, PlacementIdx(2), cell),
        "C reaches A's cell through B — an equivalence, which is what \
         MeetTogether means and what a permission must never become"
    );
}

// ---------------------------------------------------------------------------
// What actually authorizes sharing today
// ---------------------------------------------------------------------------

#[test]
fn sharing_is_authorized_by_an_exact_span_not_by_membership() {
    // Membership in a relation with a LIVE anchor this week is not enough:
    // the candidate's span must equal the anchor exactly. That is what makes
    // the untracked-occupant shortcut sound — and it is precisely the property
    // `CanShareRoom` would have to give up, since a permission with no time
    // binding has no exact span to check against.
    let problem = overlapping_relations(vec![room_with_capacity("r0", 100)], 10);
    let mut state = SearchState::from_fixed(&problem);

    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(
        !state.can_place(&problem, PlacementIdx(1), Placement::single(SlotIdx(1), RoomIdx(0))),
        "a fellow member at a DIFFERENT slot is refused even though its relation \
         has an anchor in this week — the span authorizes, not the membership"
    );
}

#[test]
fn a_relation_member_is_never_entitled_to_a_footprint_sibling() {
    // The negative counterpart to the existing positive test that members may
    // share a Room which happens to carry a footprint tag. Here the sibling
    // asks for the COMBINED Room over the same span: refused, because the
    // exemption is per identical Room and never per physical footprint.
    //
    // This is the assertion that would catch someone "simplifying" the
    // exemption to walk `footprint_siblings` — the shortcut ADR-0022's third
    // addendum exists to forbid.
    let rooms = footprint_rooms("audimax", &["r0", "r1"]);
    let problem = overlapping_relations(rooms, 10);
    let mut state = SearchState::from_fixed(&problem);

    assert!(state.place(&problem, PlacementIdx(0), Placement::single(SlotIdx(0), RoomIdx(0))));

    assert!(
        !state.can_place(&problem, PlacementIdx(1), Placement::single(SlotIdx(0), RoomIdx(1))),
        "r1 shares r0's physical space, so it is BLOCKED rather than shared: the \
         MeetTogether exemption covers the same Room, not the same footprint"
    );
}

// ---------------------------------------------------------------------------
// The capacity-relief reading has no representation, and it is a Room axis
// ---------------------------------------------------------------------------

#[test]
fn no_physical_room_can_host_two_unrelated_sessions_at_once_today() {
    // The third reading of `CanShareRoom` — "this hall seats two classes at
    // once" — is not a relation at all. `Room::is_exclusive()` is
    // `!is_virtual`, so every physical Room is exclusive no matter how many
    // seats it has spare, and `is_virtual` cannot be borrowed for it because
    // it also means "online".
    //
    // ADR-0022 already named this missing field; its virtual half was closed
    // by `MaxConcurrentOnlineSessions`, and the physical equivalent is the
    // same shape per Room. That makes it a Room axis, which is where ADR-0035
    // relocates it — not a rule about a pair of Offerings.
    let mut spec = ProblemSpec {
        rooms: vec![room_with_capacity("r0", 1000)],
        offerings: vec![
            with_min_capacity(offering("a", 1, &[0]), 10),
            with_min_capacity(offering("b", 1, &[0]), 10),
        ],
        // No relation: two ordinary, unrelated Offerings.
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let mut state = SearchState::from_fixed(&problem);
    let cell = Placement::single(SlotIdx(0), RoomIdx(0));
    assert!(state.place(&problem, PlacementIdx(0), cell));

    assert!(
        !state.can_place(&problem, PlacementIdx(1), cell),
        "980 seats to spare and still exclusive — the capacity-relief reading of \
         CanShareRoom has no representation, and it is a property of the Room"
    );
}
