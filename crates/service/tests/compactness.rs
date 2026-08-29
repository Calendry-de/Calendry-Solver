//! `Compactness`: minimize idle blocks between a Group's or Person's first and
//! last Session of a day. The wire's `repeated CompactnessScope` (empty = both
//! axes) has to resolve correctly into `Problem::compactness_group_weight` /
//! `compactness_person_weight` — that resolution is what these tests cover;
//! the gap-counting mechanism itself is tested in `calendry_solver_core::
//! aggregates`.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn compactness(scope: Vec<i32>, weight: f64) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-compact",
            pb::constraint_config::Params::Compactness(pb::Compactness { scope }),
        )
    }
}

#[test]
fn an_empty_scope_configures_both_axes() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(compactness(vec![], 3.0));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.compactness_group_weight, 3.0);
    assert_eq!(problem.compactness_person_weight, 3.0);
}

#[test]
fn a_group_only_scope_leaves_the_person_axis_unweighted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input
        .constraints
        .push(compactness(vec![pb::CompactnessScope::Group as i32], 3.0));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.compactness_group_weight, 3.0);
    assert_eq!(problem.compactness_person_weight, 0.0);
}

#[test]
fn a_person_only_scope_leaves_the_group_axis_unweighted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input
        .constraints
        .push(compactness(vec![pb::CompactnessScope::Person as i32], 3.0));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.compactness_group_weight, 0.0);
    assert_eq!(problem.compactness_person_weight, 3.0);
}

#[test]
fn two_instances_can_weight_each_axis_differently() {
    // The product's own answer to "different weights per axis": configure two
    // instances rather than growing a second weight field on one.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(pb::ConstraintConfig {
        id: "c-compact-group".into(),
        ..compactness(vec![pb::CompactnessScope::Group as i32], 2.0)
    });
    input.constraints.push(pb::ConstraintConfig {
        id: "c-compact-person".into(),
        ..compactness(vec![pb::CompactnessScope::Person as i32], 5.0)
    });

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.compactness_group_weight, 2.0);
    assert_eq!(problem.compactness_person_weight, 5.0);
}

#[test]
fn a_zero_weight_is_accepted_not_an_error() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(compactness(vec![], 0.0));

    let problem = convert(&input, &scope(&["o1"])).expect("zero is a valid weight");
    assert_eq!(problem.compactness_group_weight, 0.0);
    assert_eq!(problem.compactness_person_weight, 0.0);
}
