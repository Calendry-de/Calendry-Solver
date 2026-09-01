//! `MinimizeSpecializedRoomUse` at the wire boundary: `Room.is_specialized`
//! reaching core, the constraint resolving to an instance, and the exemption
//! being read from BOTH wire feature lists.
//!
//! The cost function itself is exercised in `crates/core`'s own test file.

use calendry_solver_core::ids::{OfferingIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::search::recompute_objective;
use calendry_solver_core::{Placement, Solution};

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, room, scope, specialized_room};

const WEIGHT: f64 = 7.0;

fn specialized_use(weight: f64) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-specialized",
            pb::constraint_config::Params::MinimizeSpecializedRoomUse(
                pb::MinimizeSpecializedRoomUse {},
            ),
        )
    }
}

/// r0 ordinary, r1 a computer lab. One Offering, eligible for both.
fn lab_input(o: pb::Offering) -> pb::SolverInput {
    let mut input = base_input();
    input.rooms = vec![room(0), specialized_room(1, &["computers"])];
    input.offerings = vec![o];
    input.constraints.push(specialized_use(WEIGHT));
    input
}

/// What placing the single Offering in `room` costs.
fn cost_in(input: &pb::SolverInput, room: u32) -> f64 {
    let problem = convert(input, &scope(&["o1"])).expect("valid input");
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(room))));
    recompute_objective(&problem, &solution).soft
}

#[test]
fn is_specialized_reaches_the_core_room() {
    let input = lab_input(offering("o1", 1));
    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert!(!problem.rooms[0].is_specialized);
    assert!(problem.rooms[1].is_specialized);
}

#[test]
fn the_constraint_resolves_and_charges_teaching_that_needs_nothing() {
    let input = lab_input(offering("o1", 1));
    assert_eq!(cost_in(&input, 1), WEIGHT, "the lab costs");
    assert_eq!(cost_in(&input, 0), 0.0, "the ordinary room does not");
}

#[test]
fn required_room_features_exempts_through_the_presence_list() {
    let input = lab_input(pb::Offering {
        required_room_features: vec!["computers".into()],
        ..offering("o1", 1)
    });
    assert_eq!(cost_in(&input, 1), 0.0);
}

#[test]
fn room_feature_requirements_exempts_through_the_quantity_list_too() {
    // The two wire lists are separate fields a caller is not required to keep
    // in sync — `build_offerings` honors both for ELIGIBILITY, so the
    // exemption must honor both as well, or an Offering that stated its need
    // in the quantity list alone would be charged for the Room it needs.
    let input = lab_input(pb::Offering {
        room_feature_requirements: vec![pb::RoomFeatureRequirement {
            feature: "computers".into(),
            min_quantity: Some(1),
        }],
        ..offering("o1", 1)
    });
    assert_eq!(cost_in(&input, 1), 0.0);
}

#[test]
fn an_unconfigured_constraint_charges_nothing_even_with_a_specialized_room() {
    let mut input = lab_input(offering("o1", 1));
    input.constraints.retain(|c| c.id != "c-specialized");
    assert_eq!(cost_in(&input, 1), 0.0);
}

#[test]
fn a_disabled_instance_charges_nothing() {
    let mut input = lab_input(offering("o1", 1));
    for c in &mut input.constraints {
        if c.id == "c-specialized" {
            c.enabled = false;
        }
    }
    assert_eq!(cost_in(&input, 1), 0.0);
}

#[test]
fn a_negative_weight_is_refused() {
    // Same reason every other soft weight must be >= 0: the type declares
    // minimize, so a negative weight would REWARD filling the lab.
    let mut input = lab_input(offering("o1", 1));
    input.constraints.retain(|c| c.id != "c-specialized");
    input.constraints.push(specialized_use(-1.0));

    let e = convert(&input, &scope(&["o1"])).expect_err("a negative weight inverts the term");
    assert!(
        matches!(&e, ConvertError::NegativeSoftWeight { constraint, .. }
                     if constraint == "c-specialized"),
        "got {e}"
    );
}

#[test]
fn marking_a_room_specialized_does_not_change_its_eligibility() {
    // The mark PRICES a Room; it never filters one. An Offering requiring
    // nothing is still eligible for the lab, which is exactly why a soft term
    // is needed to steer it away.
    let input = lab_input(offering("o1", 1));
    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(
        problem.offerings[OfferingIdx(0).get()].eligible_rooms,
        vec![RoomIdx(0), RoomIdx(1)],
        "both rooms remain eligible"
    );
}
