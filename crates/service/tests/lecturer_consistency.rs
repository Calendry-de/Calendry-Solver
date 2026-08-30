//! `LecturerConsistency` at the wire boundary: an empty message, mirroring
//! `RoomConsistency` — no field to parse beyond the enclosing
//! `ConstraintConfig`'s id/kinds/weight.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn lecturer_consistency(weight: f64, kinds: Vec<String>) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        applies_to_kinds: kinds,
        ..enabled(
            "c-lecturer-consistency",
            pb::constraint_config::Params::LecturerConsistency(pb::LecturerConsistency {}),
        )
    }
}

#[test]
fn kinds_and_weight_reach_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input
        .constraints
        .push(lecturer_consistency(4.0, vec!["lecture".into()]));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.lecturer_consistency_weight, 4.0);
    assert_eq!(problem.constraints.lecturer_consistency[0].kinds, vec!["lecture".to_string()]);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(lecturer_consistency(-1.0, vec![]));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}
