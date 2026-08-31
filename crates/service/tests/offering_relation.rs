//! `OfferingRelation` / `DifferentTime` at the wire boundary (issue #50):
//! resolving `offering_ids` to a real membership list, and refusing the
//! structural faults a dangling or too-small relation would otherwise hide.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, offering, scope};

fn relation(id: &str, offering_ids: &[&str]) -> pb::OfferingRelation {
    pb::OfferingRelation {
        id: id.into(),
        enabled: true,
        weight: 0.0,
        offering_ids: offering_ids.iter().map(|s| (*s).to_string()).collect(),
        params: Some(pb::offering_relation::Params::DifferentTime(pb::DifferentTime {})),
    }
}

fn two_related_offerings(input: &mut pb::SolverInput) {
    input.offerings = vec![offering("o1", 0), offering("o2", 0)];
}

#[test]
fn membership_reaches_both_offerings() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![relation("rel-1", &["o1", "o2"])];

    let problem = convert(&input, &scope(&["o1", "o2"])).expect("valid input");
    assert_eq!(problem.offerings[0].different_time_relations, vec![0]);
    assert_eq!(problem.offerings[1].different_time_relations, vec![0]);
}

#[test]
fn a_disabled_relation_reaches_neither_offering() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations =
        vec![pb::OfferingRelation { enabled: false, ..relation("rel-1", &["o1", "o2"]) }];

    let problem = convert(&input, &scope(&["o1", "o2"])).expect("valid input");
    assert!(problem.offerings[0].different_time_relations.is_empty());
    assert!(problem.offerings[1].different_time_relations.is_empty());
}

#[test]
fn an_unresolvable_offering_id_is_refused() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![relation("rel-1", &["o1", "does-not-exist"])];

    let e = convert(&input, &scope(&["o1", "o2"])).expect_err("dangling id must be refused");
    assert!(
        matches!(e, ConvertError::UnknownOffering { offering, .. } if offering == "does-not-exist")
    );
}

#[test]
fn a_relation_naming_fewer_than_two_offerings_is_refused() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![relation("rel-1", &["o1"])];

    let e = convert(&input, &scope(&["o1", "o2"])).expect_err("a lone member is meaningless");
    assert!(matches!(e, ConvertError::RelationTooFewMembers { members: 1, .. }));
}

#[test]
fn a_relation_without_params_is_refused() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations =
        vec![pb::OfferingRelation { params: None, ..relation("rel-1", &["o1", "o2"]) }];

    let e = convert(&input, &scope(&["o1", "o2"])).expect_err("no params set must be refused");
    assert!(matches!(e, ConvertError::RelationWithoutParams { .. }));
}

// ---------------------------------------------------------------------------
// SameTime / SameDays / SameStart (issue #54) are built: wire params resolve
// to the matching `RelationKind`, same membership-resolution path as
// `DifferentTime` above. The evaluator itself (full day/block-set equality,
// HARD but priced) is exercised in `crates/core`'s own tests, not here.
// ---------------------------------------------------------------------------

#[test]
fn same_time_resolves_to_same_time_kind() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![pb::OfferingRelation {
        params: Some(pb::offering_relation::Params::SameTime(pb::SameTime {})),
        ..relation("rel-1", &["o1", "o2"])
    }];

    let problem = convert(&input, &scope(&["o1", "o2"])).expect("valid input");
    assert_eq!(problem.relations.len(), 1);
    assert_eq!(problem.relations[0].kind, calendry_solver_core::problem::RelationKind::SameTime);
}

#[test]
fn same_days_resolves_to_same_days_kind() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![pb::OfferingRelation {
        params: Some(pb::offering_relation::Params::SameDays(pb::SameDays {})),
        ..relation("rel-1", &["o1", "o2"])
    }];

    let problem = convert(&input, &scope(&["o1", "o2"])).expect("valid input");
    assert_eq!(problem.relations.len(), 1);
    assert_eq!(problem.relations[0].kind, calendry_solver_core::problem::RelationKind::SameDays);
}

#[test]
fn same_start_resolves_to_same_start_kind() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![pb::OfferingRelation {
        params: Some(pb::offering_relation::Params::SameStart(pb::SameStart {})),
        ..relation("rel-1", &["o1", "o2"])
    }];

    let problem = convert(&input, &scope(&["o1", "o2"])).expect("valid input");
    assert_eq!(problem.relations.len(), 1);
    assert_eq!(problem.relations[0].kind, calendry_solver_core::problem::RelationKind::SameStart);
}

// ---------------------------------------------------------------------------
// MeetTogether (issue #55) is staged schema-first, evaluator not yet built
// — see the solver repo's CLAUDE.md for what is actually implemented.
// ---------------------------------------------------------------------------

#[test]
fn meet_together_is_unimplemented_not_invalid() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![pb::OfferingRelation {
        params: Some(pb::offering_relation::Params::MeetTogether(pb::MeetTogether {})),
        ..relation("rel-1", &["o1", "o2"])
    }];

    let e = convert(&input, &scope(&["o1", "o2"])).expect_err("not yet built");
    assert!(matches!(
        e,
        ConvertError::RelationKindUnimplemented { relation_kind: "MeetTogether", .. }
    ));
}
