//! `Daybreak` at the wire boundary: `CompactnessScope` parses the same way
//! `MaxConsecutiveBlocks` does, and weight IS validated — SOFT, like
//! `RoomTurnaroundBuffer`, unlike `MaxDays`/`MaxConsecutiveDays`.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn daybreak(weight: f64, scope_axes: Vec<i32>, min_rest_minutes: u32) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-daybreak",
            pb::constraint_config::Params::Daybreak(pb::Daybreak {
                scope: scope_axes,
                min_rest_minutes,
            }),
        )
    }
}

#[test]
fn an_empty_scope_means_both_axes() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(daybreak(5.0, vec![], 600));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.constraints.daybreak.len(), 1);
    assert!(problem.constraints.daybreak[0].group);
    assert!(problem.constraints.daybreak[0].person);
    assert_eq!(problem.constraints.daybreak[0].min_rest_minutes, 600);
}

#[test]
fn a_person_only_scope_leaves_the_group_axis_unset() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input
        .constraints
        .push(daybreak(5.0, vec![pb::CompactnessScope::Person as i32], 600));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert!(!problem.constraints.daybreak[0].group);
    assert!(problem.constraints.daybreak[0].person);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(daybreak(-1.0, vec![], 600));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(e, ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0));
}

#[test]
fn a_zero_weight_is_accepted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(daybreak(0.0, vec![], 600));

    let problem = convert(&input, &scope(&["o1"])).expect("zero is a valid weight");
    assert_eq!(problem.constraints.daybreak[0].weight, 0.0);
}
