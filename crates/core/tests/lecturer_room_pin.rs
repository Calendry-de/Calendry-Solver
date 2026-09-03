//! `LecturerRoomPin`: the workshop lead always teaches in the workshop.
//!
//! `Person::allowed_rooms` is a HARD restriction on the Rooms a Person may
//! LEAD a Session in. Mechanically it is small — one bit test in
//! `statically_blocked`. What needs pinning is the one decision that makes it
//! work at all, plus two inversions that look right and are not:
//!
//! * **The question is asked against the placement's CHOSEN lecturers**, never
//!   a mask precomputed onto the Offering. `LecturerVeto` is the tempting
//!   sibling to copy and is the wrong one: its mask is built from
//!   `Offering::lecturers`, which is EMPTY for a genuine lecturer pool — which
//!   is why `LecturerVeto` plus a pool has to be refused at conversion, and
//!   why copying its shape would be silently permissive for exactly the case
//!   this rule exists to serve. `a_pin_binds_a_fixed_assignment` /
//!   `a_pin_binds_a_pool_offering` are the mirrored pair that catches it, built
//!   the way ADR-0027 builds its expansion guard: the first passes under both
//!   implementations and exists only so the second is not vacuously green.
//! * **An empty pin is NO pin, not NO rooms.** The wire's whitelist has empty
//!   meaning "everything"; every mask in `core` has empty meaning "nothing".
//!   `Problem::build` inverts once, and `an_empty_pin_is_no_pin_not_no_rooms`
//!   is red against an inversion that forgot the empty case.
//! * **A pin does not expand through a footprint.** ADR-0022 expands a
//!   BLOCKING question — booking one identity of a folding-wall space consumes
//!   the others — and a permission never expands.
//!
//! Assertions are mostly `is_free` rather than costs, because this is a filter:
//! the same choice `room_footprint.rs` makes, and for the same reason.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::ids::{PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ProblemSpec, Room};
use calendry_solver_core::solution::{Occupant, Placement, SearchState};
use calendry_solver_core::testing::{self, person_pinned_to, with_room_pin};
use calendry_solver_core::{Problem, Solution};

mod common;
use common::solve_with_move_budget as run;

/// Is the only Offering allowed to occupy `rooms` at slot 0, according to the
/// filter the search itself consults?
///
/// `SearchState::from_fixed` plus `is_free` asks the real question rather than
/// a reimplementation of it — a pin that only showed up in `evaluate_hard`
/// would let construction land in a barred Room and then report it, instead of
/// never going there.
fn free_in(problem: &Problem, rooms: &[u32], lecturers: &[u32]) -> bool {
    let state = SearchState::from_fixed(problem);
    let offering = &problem.offerings[0];
    let span = problem
        .slots
        .span(SlotIdx(0), offering.duration_blocks)
        .expect("slot in grid");

    let mut occupant = Occupant::of_offering(offering).with_room(RoomIdx(rooms[0]));
    if rooms.len() > 1 {
        let mut extra = [None; 3];
        for (i, &r) in rooms[1..].iter().enumerate() {
            extra[i] = Some(RoomIdx(r));
        }
        occupant = occupant.with_additional_rooms(extra);
    }
    if !lecturers.is_empty() {
        let mut chosen = [None; 4];
        for (i, &l) in lecturers.iter().enumerate() {
            chosen[i] = Some(PersonIdx(l));
        }
        occupant = occupant.with_pool_lecturers(chosen);
    }

    state.is_free(problem, &occupant, &span)
}

/// One Offering needing one Session over `n_rooms` Rooms, led by a fixed
/// assignment of person 0, with the pin enabled for every kind.
fn fixed_assignment(n_rooms: u32, pin: &[u32]) -> Problem {
    let eligible: Vec<u32> = (0..n_rooms).collect();
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(n_rooms),
        persons: vec![person_pinned_to("P0", pin)],
        offerings: vec![testing::with_lecturers(
            testing::offering("S", 1, &eligible),
            &[0],
        )],
        constraints: with_room_pin(testing::all_constraints(), &[]),
        ..ProblemSpec::new(testing::grid(1, 1))
    })
}

// ---------------------------------------------------------------------------
// The guard pair — one fixture, two mirrored assertions (§ADR-0034)
// ---------------------------------------------------------------------------

#[test]
fn a_pin_binds_a_fixed_assignment() {
    // Passes under the exact check AND under a `LecturerVeto`-style mask
    // precomputed from `Offering::lecturers`, which is populated here. Present
    // only so its mirror below is not vacuously green.
    let problem = fixed_assignment(3, &[1]);

    assert!(!free_in(&problem, &[0], &[]), "R0 is not in the pin");
    assert!(free_in(&problem, &[1], &[]), "R1 is");
    assert!(!free_in(&problem, &[2], &[]), "R2 is not");
}

#[test]
fn a_pin_binds_a_pool_offering() {
    // THE DISCRIMINATING TEST. A pool Offering's `Offering::lecturers` is
    // EMPTY — the assignment is a search-time choice carried on the Placement
    // — so a per-Offering mask copied from `LecturerVeto` would be empty too
    // and would permit every Room. Only a check against the CHOSEN lecturers
    // gets this right.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(3),
        persons: vec![person_pinned_to("P0", &[1]), person_pinned_to("P1", &[2])],
        offerings: vec![testing::with_lecturer_pool(
            testing::offering("S", 1, &[0, 1, 2]),
            1,
            &[0, 1],
        )],
        constraints: with_room_pin(testing::all_constraints(), &[]),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    assert!(!free_in(&problem, &[0], &[0]), "P0 may not use R0");
    assert!(free_in(&problem, &[1], &[0]), "P0 may use R1");
    assert!(!free_in(&problem, &[2], &[0]), "P0 may not use R2");

    // The same Room, the other candidate: the answer must depend on WHO was
    // chosen, which is the whole claim.
    assert!(!free_in(&problem, &[1], &[1]), "P1 may not use R1");
    assert!(free_in(&problem, &[2], &[1]), "P1 may use R2");
}

// ---------------------------------------------------------------------------
// The two inversions
// ---------------------------------------------------------------------------

#[test]
fn an_empty_pin_is_no_pin_not_no_rooms() {
    // RED against a complement built without its empty guard, which would
    // blacklist every Room and make one unconfigured Person unplaceable
    // everywhere. The wire's whitelist has empty meaning EVERYTHING; the
    // complement stored here has empty meaning NOTHING; the inversion happens
    // once, in `Problem::build`.
    let problem = fixed_assignment(3, &[]);

    for r in 0..3 {
        assert!(free_in(&problem, &[r], &[]), "R{r} must stay free with no pin stated");
    }
    let outcome = run(&problem);
    assert!(outcome.solution.get(PlacementIdx(0)).is_some(), "and the Session places");
}

#[test]
fn a_pin_does_not_expand_through_a_footprint() {
    // ADR-0022 expands a footprint on the QUERY side of a BLOCKING question:
    // booking sub-room R0 consumes the Audimax R3 that subsumes it. A
    // PERMISSION never expands — being allowed in R0 says nothing about being
    // allowed in R3 — so the pin must not walk the sibling set.
    let mut rooms: Vec<Room> = testing::footprint_rooms("audimax", &["R0", "R1", "R2", "R3"]);
    rooms.push(testing::room("R4"));

    let problem = testing::assemble(ProblemSpec {
        rooms,
        persons: vec![person_pinned_to("P0", &[0])],
        offerings: vec![testing::with_lecturers(
            testing::offering("S", 1, &[0, 1, 2, 3, 4]),
            &[0],
        )],
        constraints: with_room_pin(testing::all_constraints(), &[]),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    assert!(free_in(&problem, &[0], &[]), "the pinned sub-room");
    assert!(
        !free_in(&problem, &[3], &[]),
        "the combined Room shares R0's physical space but is NOT in the pin — a \
         permission that expanded through the footprint would admit it"
    );
    assert!(!free_in(&problem, &[1], &[]), "nor is a sibling sub-room");
}

// ---------------------------------------------------------------------------
// Scope, inertness, and the multi-room reading
// ---------------------------------------------------------------------------

#[test]
fn an_unpinned_person_is_placed_anywhere() {
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(3),
        persons: vec![testing::person("P0", &[])],
        offerings: vec![testing::with_lecturers(
            testing::offering("S", 1, &[0, 1, 2]),
            &[0],
        )],
        constraints: with_room_pin(testing::all_constraints(), &[]),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    for r in 0..3 {
        assert!(free_in(&problem, &[r], &[]), "R{r}");
    }
}

#[test]
fn a_pin_is_inert_until_the_rule_is_enabled() {
    // The `LecturerVeto`/`GroupVeto` split: the values are Person data, the
    // switch is tenant policy. A tenant that stores pins but has not enabled
    // the rule gets today's behaviour exactly.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(3),
        persons: vec![person_pinned_to("P0", &[1])],
        offerings: vec![testing::with_lecturers(
            testing::offering("S", 1, &[0, 1, 2]),
            &[0],
        )],
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    for r in 0..3 {
        assert!(free_in(&problem, &[r], &[]), "R{r} with the rule disabled");
    }
}

#[test]
fn a_pin_is_scoped_by_kind() {
    // Enabled for `lecture` only. The fixture's Offering kind is `lecture`,
    // so a rule scoped to `staff_meeting` must not touch it.
    let build = |kinds: &[&str]| {
        testing::assemble(ProblemSpec {
            rooms: testing::rooms(2),
            persons: vec![person_pinned_to("P0", &[1])],
            offerings: vec![testing::with_lecturers(
                testing::offering("S", 1, &[0, 1]),
                &[0],
            )],
            constraints: with_room_pin(testing::all_constraints(), kinds),
            ..ProblemSpec::new(testing::grid(1, 1))
        })
    };

    assert!(!free_in(&build(&["lecture"]), &[0], &[]), "covered: R0 is barred");
    assert!(free_in(&build(&["staff_meeting"]), &[0], &[]), "not covered: inert");
}

#[test]
fn a_pinned_person_who_only_attends_is_not_constrained() {
    // ADR-0026's scope decision, one rule over: the pin counts Sessions the
    // Person LEADS, never ones they attend. A workshop lead who also sits in
    // staff meetings must not have those meetings pinned.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        persons: vec![testing::person("P0", &[]), person_pinned_to("P1", &[1])],
        offerings: vec![calendry_solver_core::problem::OfferingSpec {
            // Person 1 ATTENDS this Session; person 0 leads it. There is no
            // `with_participants` fixture, so the field is set directly.
            participants: vec![PersonIdx(1)],
            ..testing::with_lecturers(testing::offering("S", 1, &[0, 1]), &[0])
        }],
        constraints: with_room_pin(testing::all_constraints(), &[]),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    assert!(
        free_in(&problem, &[0], &[]),
        "P1 attends but does not lead, so their pin says nothing about this Room"
    );
}

#[test]
fn every_room_of_a_multi_room_session_must_satisfy_the_pin() {
    // "At least one" would let a hard pin be escaped by requiring MORE Rooms,
    // which is the reading to avoid.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(3),
        persons: vec![person_pinned_to("P0", &[0, 1])],
        offerings: vec![testing::with_room_combinations(
            testing::with_lecturers(testing::offering("S", 1, &[0, 1, 2]), &[0]),
            2,
            &[0, 1, 2],
        )],
        constraints: with_room_pin(testing::all_constraints(), &[]),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    assert!(free_in(&problem, &[0, 1], &[]), "both Rooms are in the pin");
    assert!(
        !free_in(&problem, &[0, 2], &[]),
        "R2 is not, and one permitted Room does not license the pair"
    );
}

// ---------------------------------------------------------------------------
// It is a filter, and it is reported
// ---------------------------------------------------------------------------

#[test]
fn a_satisfied_pin_moves_no_soft_term() {
    // Filter, not price. The rule adds no objective term at all, which is why
    // no aggregate-drift or soft-search test needed changing for it.
    let pinned = fixed_assignment(2, &[0]);
    let unpinned = fixed_assignment(2, &[]);

    assert_eq!(
        run(&pinned).objective.soft,
        run(&unpinned).objective.soft,
        "a satisfied pin must cost exactly nothing"
    );
}

#[test]
fn the_pin_is_reported_for_a_placement_that_violates_it() {
    // ADR-0014: the authoritative check shares no code path with the occupancy
    // index, so it can catch the index being wrong. The search cannot produce
    // this Solution — it is built by hand precisely because the filter would
    // have refused it.
    let problem = fixed_assignment(2, &[1]);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));

    let violations = evaluate_hard(&problem, &solution);
    let pin: Vec<_> = violations
        .iter()
        .filter(|v| v.constraint_type == ConstraintType::LecturerRoomPin)
        .collect();

    assert_eq!(pin.len(), 1, "one breach, reported once: {violations:?}");
    // ADR-0027's naming lesson: the Session looks ordinary, so the report has
    // to name whoever declared the rule and the Room they may not use.
    assert!(pin[0].detail.contains("P0"), "must name the Person: {}", pin[0].detail);
    assert!(pin[0].detail.contains("R0"), "must name the Room: {}", pin[0].detail);
}
