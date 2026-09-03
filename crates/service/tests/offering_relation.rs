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
// MeetTogether (issue #55) is built: wire params resolve to
// `RelationKind::MeetTogether`, same membership-resolution path as
// `DifferentTime` above. The Room-sharing/capacity mechanism itself is
// exercised in `crates/core`'s own tests, not here.
// ---------------------------------------------------------------------------

#[test]
fn meet_together_resolves_to_meet_together_kind() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![pb::OfferingRelation {
        params: Some(pb::offering_relation::Params::MeetTogether(pb::MeetTogether {})),
        ..relation("rel-1", &["o1", "o2"])
    }];

    let problem = convert(&input, &scope(&["o1", "o2"])).expect("valid input");
    assert_eq!(problem.offerings[0].meet_together_relations, vec![0]);
    assert_eq!(problem.offerings[1].meet_together_relations, vec![0]);
}

// ---------------------------------------------------------------------------
// Precedence (issue #37) is built, and is the one kind carrying PARAMETERS.
// Both `u32`s pass through unvalidated on purpose — every value is
// meaningful, the two zeroes included. The evaluator (term-wide all-pairs
// ordering, the wall-clock gap, the calendar-day ceiling) is exercised in
// `crates/core`'s own tests, not here.
// ---------------------------------------------------------------------------

fn with_precedence(min_gap_minutes: u32, max_days_between: u32) -> pb::OfferingRelation {
    pb::OfferingRelation {
        params: Some(pb::offering_relation::Params::Precedence(pb::Precedence {
            min_gap_minutes,
            min_days_between: 0,
            max_days_between,
        })),
        ..relation("rel-1", &["o1", "o2"])
    }
}

#[test]
fn precedence_resolves_to_precedence_kind_carrying_both_parameters() {
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![with_precedence(1_440, 7)];

    let problem = convert(&input, &scope(&["o1", "o2"])).expect("valid input");
    assert_eq!(problem.relations.len(), 1);
    assert_eq!(
        problem.relations[0].kind,
        calendry_solver_core::problem::RelationKind::Precedence {
            min_gap_minutes: 1_440,
            min_days_between: 0,
            max_days_between: 7,
        }
    );
}

#[test]
fn precedence_keeps_its_members_in_the_configured_order() {
    // The only kind that reads the order, so the conversion must not sort,
    // dedupe or otherwise normalize `offering_ids` into a set.
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![pb::OfferingRelation {
        offering_ids: vec!["o2".into(), "o1".into()],
        ..with_precedence(0, 0)
    }];

    let problem = convert(&input, &scope(&["o1", "o2"])).expect("valid input");
    let members = &problem.relations[0].members;
    assert_eq!(
        problem.offerings[members[0].get()].id,
        "o2",
        "the predecessor is whichever Offering the caller listed first"
    );
    assert_eq!(problem.offerings[members[1].get()].id, "o1");
}

#[test]
fn both_precedence_parameters_at_zero_are_accepted_not_treated_as_unset() {
    // 0 / 0 means "back-to-back is fine, no upper bound" — a real
    // configuration, and the one the app sends for a bare ordering rule.
    let mut input = base_input();
    two_related_offerings(&mut input);
    input.offering_relations = vec![with_precedence(0, 0)];

    let problem = convert(&input, &scope(&["o1", "o2"])).expect("valid input");
    assert_eq!(
        problem.relations[0].kind,
        calendry_solver_core::problem::RelationKind::Precedence {
            min_gap_minutes: 0,
            min_days_between: 0,
            max_days_between: 0,
        }
    );
}
