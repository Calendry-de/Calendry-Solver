//! `MinimizeSpecializedRoomUse`: keep a lab, computer room or workshop free
//! for the teaching that actually needs it.
//!
//! Two decisions carry the whole type, and each would fail silently if it
//! regressed:
//!
//! 1. **Exempt by REQUIREMENT, not by configuration.** An Offering requiring
//!    any of that Room's features pays nothing — the programming class in the
//!    computer lab is exactly where it belongs, and charging it would price a
//!    choice it never had.
//! 2. **A separate axis from `Room::rank`.** A Room can be specialized,
//!    premium, both or neither, and `MinimizeRoomRank` must not be able to
//!    see this field or vice versa.

use calendry_solver_core::ids::{OfferingIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{Problem, ProblemSpec};
use calendry_solver_core::search::recompute_objective;
use calendry_solver_core::testing::{
    fixture, grid, offering, requiring_features, room, specialized_room, structural_room_only,
    with_specialized_room_use,
};
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

const WEIGHT: f64 = 7.0;

/// Room 0 is an ordinary room; room 1 is a computer lab. One Offering, which
/// requires `requires` and may use either room.
fn lab_problem(requires: &[&str], weight: f64) -> Problem {
    let o = requiring_features(offering("o", 1, &[0, 1]), requires);
    let mut spec = ProblemSpec {
        rooms: vec![room("plain"), specialized_room("lab", &["computers"])],
        offerings: vec![o],
        ..fixture(grid(2, 1), with_specialized_room_use(structural_room_only(), weight))
    };
    spec.expand_placements();
    Problem::build(spec).unwrap()
}

/// What the single placement costs in `room`.
fn cost_in(problem: &Problem, room: u32) -> f64 {
    let mut solution = Solution::empty(problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(room))));
    recompute_objective(problem, &solution).soft
}

#[test]
fn teaching_that_needs_nothing_is_charged_for_taking_the_lab() {
    let problem = lab_problem(&[], WEIGHT);
    assert_eq!(cost_in(&problem, 1), WEIGHT);
}

#[test]
fn the_same_teaching_costs_nothing_in_an_ordinary_room() {
    let problem = lab_problem(&[], WEIGHT);
    assert_eq!(cost_in(&problem, 0), 0.0);
}

#[test]
fn teaching_that_requires_the_labs_feature_is_exempt() {
    // The decision that makes this type usable at all: the programming class
    // belongs in the computer lab, and charging it would either be noise on
    // the objective or push it out of the only Room that suits it.
    let problem = lab_problem(&["computers"], WEIGHT);
    assert_eq!(cost_in(&problem, 1), 0.0);
}

#[test]
fn requiring_a_feature_the_lab_does_not_have_does_not_exempt() {
    // Exemption is an intersection, not "requires anything at all".
    let problem = lab_problem(&["piano"], WEIGHT);
    assert_eq!(cost_in(&problem, 1), WEIGHT);
}

#[test]
fn requiring_any_one_of_the_rooms_features_is_enough() {
    let o = requiring_features(offering("o", 1, &[0, 1]), &["computers"]);
    let mut spec = ProblemSpec {
        rooms: vec![
            room("plain"),
            specialized_room("lab", &["computers", "projector"]),
        ],
        offerings: vec![o],
        ..fixture(grid(2, 1), with_specialized_room_use(structural_room_only(), WEIGHT))
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();
    assert_eq!(cost_in(&problem, 1), 0.0, "one shared feature is enough");
}

#[test]
fn an_unmarked_room_is_never_charged_however_many_features_it_has() {
    // The mark is what makes a Room scarce, NOT how richly it is tagged —
    // otherwise the penalty would track how thoroughly a tenant filled in
    // `feature_tags`, which is not a scheduling fact.
    let mut spec = ProblemSpec {
        rooms: vec![
            room("plain"),
            calendry_solver_core::problem::Room {
                features: vec!["computers".into(), "projector".into()],
                ..room("well-equipped")
            },
        ],
        offerings: vec![offering("o", 1, &[0, 1])],
        ..fixture(grid(2, 1), with_specialized_room_use(structural_room_only(), WEIGHT))
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();
    assert_eq!(cost_in(&problem, 1), 0.0);
}

#[test]
fn nothing_is_charged_when_the_constraint_is_not_configured() {
    let problem = lab_problem(&[], 0.0);
    assert_eq!(cost_in(&problem, 1), 0.0);

    // And with no instance at all, not merely a zero weight.
    let mut spec = ProblemSpec {
        rooms: vec![room("plain"), specialized_room("lab", &["computers"])],
        offerings: vec![offering("o", 1, &[0, 1])],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let unconfigured = Problem::build(spec).unwrap();
    assert_eq!(cost_in(&unconfigured, 1), 0.0);
}

#[test]
fn the_search_prefers_the_ordinary_room_when_it_is_free() {
    // The point of the whole type: with both rooms available and nothing else
    // to separate them, the lab is left alone.
    use calendry_solver_core::search::{NeverHalt, solve};
    let problem = lab_problem(&[], WEIGHT);
    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);

    assert_eq!(outcome.objective.unplaced, 0);
    let placed = outcome
        .solution
        .get(PlacementIdx(0))
        .expect("must be placed");
    assert_eq!(placed.room, RoomIdx(0), "the lab should have been left free");
}

#[test]
fn the_lab_is_still_used_when_it_is_the_only_room_that_fits() {
    // SOFT, so it must never prevent a placement — the search takes the lab
    // and pays, rather than leaving the Session unplaced.
    let o = requiring_features(offering("o", 1, &[1]), &[]);
    let mut spec = ProblemSpec {
        rooms: vec![room("plain"), specialized_room("lab", &["computers"])],
        offerings: vec![o],
        ..fixture(grid(2, 1), with_specialized_room_use(structural_room_only(), WEIGHT))
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    use calendry_solver_core::search::{NeverHalt, solve};
    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0, "a soft term must never refuse to place");
    assert!(outcome.objective.soft >= WEIGHT, "and it pays the charge");
}

#[test]
fn the_charge_is_flat_not_scaled_by_how_many_specialized_rooms_are_taken() {
    // `is_specialized` is a boolean, so there is no gradient to grade — this
    // is what keeps one placement's ceiling equal to the summed weight and
    // `hard_penalty`'s bound exact.
    let problem = lab_problem(&[], WEIGHT);
    assert_eq!(cost_in(&problem, 1), WEIGHT);
}

#[test]
fn specialized_and_rank_are_independent_axes() {
    // A Room may be premium, specialized, both or neither. If either field
    // ever started reading the other, this would break.
    let plain = room("plain");
    let lab = specialized_room("lab", &["computers"]);
    assert_eq!(lab.rank, plain.rank, "marking a Room specialized must not change its rank");
    assert!(lab.is_specialized && !plain.is_specialized);
}

#[test]
fn hard_penalty_still_dominates_the_specialized_charge() {
    // The bound `hard_penalty` relies on is "each term costs at most its own
    // weight per placement". A flat once-per-placement charge keeps that
    // exact, but only if the weight is actually folded in.
    let problem = lab_problem(&[], 1_000.0);
    assert!(
        problem.hard_penalty > cost_in(&problem, 1),
        "hard_penalty {} must dominate one placement's charge",
        problem.hard_penalty
    );
}

#[test]
fn kind_scoping_selects_which_offerings_are_charged() {
    // Scoped like every other constraint: an instance naming other kinds
    // leaves this Offering alone.
    use calendry_solver_core::problem::MinimizeSpecializedRoomUseInstance;
    let mut set = structural_room_only();
    set.minimize_specialized_room_use = vec![MinimizeSpecializedRoomUseInstance {
        id: "c".into(),
        kinds: vec!["seminar".into()],
        weight: WEIGHT,
    }];
    let mut spec = ProblemSpec {
        rooms: vec![room("plain"), specialized_room("lab", &["computers"])],
        // `offering` builds a "lecture", which the instance does not cover.
        offerings: vec![offering("o", 1, &[0, 1])],
        ..fixture(grid(2, 1), set)
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();
    assert_eq!(cost_in(&problem, 1), 0.0);
    assert_eq!(problem.offerings[OfferingIdx(0).get()].specialized_room_charge, 0.0);
}
