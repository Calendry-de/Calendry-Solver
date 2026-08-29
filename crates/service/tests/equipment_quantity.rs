//! `Room.feature_quantities` / `Offering.room_feature_requirements`: room
//! eligibility becomes count-aware for a requirement that states a minimum,
//! instead of presence-only.
//!
//! "Equipment quantity cannot cross the wire": a 24-seat lab and a room with
//! one workstation used to be equally eligible for a practical requiring 24,
//! because `required_room_features` only asked "does the room have this tag
//! at all". These tests are the falsification target for that gap.

use calendry_solver_core::ids::OfferingIdx;

use calendry_solver::convert::convert;

mod common;
use calendry_solver_proto::v1 as pb;
use common::{base_input, offering, room, scope};

fn quantity(feature: &str, quantity: u32) -> pb::RoomFeatureQuantity {
    pb::RoomFeatureQuantity { feature: feature.into(), quantity }
}

fn requirement(feature: &str, min_quantity: Option<u32>) -> pb::RoomFeatureRequirement {
    pb::RoomFeatureRequirement { feature: feature.into(), min_quantity }
}

#[test]
fn a_room_below_the_stated_minimum_is_ineligible() {
    let mut input = base_input();
    input.rooms = vec![
        pb::Room { feature_quantities: vec![quantity("workstation", 30)], ..room(0) },
        pb::Room { feature_quantities: vec![quantity("workstation", 1)], ..room(1) },
    ];
    input.offerings = vec![pb::Offering {
        room_feature_requirements: vec![requirement("workstation", Some(24))],
        ..offering("o1", 1)
    }];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    let eligible = &problem.offerings[OfferingIdx(0).get()].eligible_rooms;

    assert!(eligible.iter().any(|r| r.get() == 0), "30 workstations satisfies a minimum of 24");
    assert!(
        !eligible.iter().any(|r| r.get() == 1),
        "1 workstation must not satisfy a minimum of 24"
    );
}

#[test]
fn a_requirement_with_no_minimum_asks_the_same_question_required_room_features_does() {
    let mut input = base_input();
    input.rooms = vec![
        // Present via `feature_quantities`, any nonzero count.
        pb::Room { feature_quantities: vec![quantity("projector", 1)], ..room(0) },
        // Present via `feature_tags`, the older presence-only vocabulary —
        // callers are not required to keep the two lists in sync.
        pb::Room { feature_tags: vec!["projector".into()], ..room(1) },
    ];
    // Room 1 is a subset of room 0's setup, minus the tag, so give the
    // offering only two candidate rooms and no other filter to confound it.
    input.offerings = vec![pb::Offering {
        room_feature_requirements: vec![requirement("projector", None)],
        ..offering("o1", 1)
    }];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    let eligible = &problem.offerings[OfferingIdx(0).get()].eligible_rooms;

    assert_eq!(eligible.len(), 2, "both a quantities-entry and a feature-tag satisfy 'no minimum'");
}

#[test]
fn required_room_features_and_room_feature_requirements_are_both_honored() {
    // A room can pass the old presence-only check and still fail the new
    // quantity-aware one — the two lists are additive, not alternatives.
    let mut input = base_input();
    input.rooms = vec![pb::Room {
        feature_tags: vec!["lab".into()],
        feature_quantities: vec![quantity("workstation", 2)],
        ..room(0)
    }];
    input.offerings = vec![pb::Offering {
        required_room_features: vec!["lab".into()],
        room_feature_requirements: vec![requirement("workstation", Some(24))],
        ..offering("o1", 1)
    }];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert!(
        problem.offerings[OfferingIdx(0).get()]
            .eligible_rooms
            .is_empty(),
        "presence of 'lab' must not paper over a genuine workstation shortfall"
    );
}

#[test]
fn a_zero_minimum_is_vacuously_satisfied() {
    let mut input = base_input();
    input.rooms = vec![room(0)]; // no feature_quantities entries at all
    input.offerings = vec![pb::Offering {
        room_feature_requirements: vec![requirement("workstation", Some(0))],
        ..offering("o1", 1)
    }];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(
        problem.offerings[OfferingIdx(0).get()].eligible_rooms.len(),
        1,
        "a minimum of zero asks for at least zero, which any Room already has"
    );
}
