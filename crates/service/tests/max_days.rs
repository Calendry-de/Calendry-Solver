//! `MaxDays` / `MaxConsecutiveDays` at the wire boundary: `CompactnessScope`
//! parses the same way `MaxConsecutiveBlocks` does, and neither validates
//! weight — HARD, like `MaxConcurrentOnlineSessions`.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn max_days(scope_axes: Vec<i32>, max_days: u32) -> pb::ConstraintConfig {
    enabled(
        "c-days",
        pb::constraint_config::Params::MaxDays(pb::MaxDays { scope: scope_axes, max_days }),
    )
}

fn max_consecutive_days(scope_axes: Vec<i32>, max_consecutive_days: u32) -> pb::ConstraintConfig {
    enabled(
        "c-consec-days",
        pb::constraint_config::Params::MaxConsecutiveDays(pb::MaxConsecutiveDays {
            scope: scope_axes,
            max_consecutive_days,
        }),
    )
}

#[test]
fn an_empty_scope_means_both_axes() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(max_days(vec![], 3));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.constraints.max_days.len(), 1);
    assert!(problem.constraints.max_days[0].group);
    assert!(problem.constraints.max_days[0].person);
    assert_eq!(problem.constraints.max_days[0].max_days, 3);
}

#[test]
fn a_group_only_scope_leaves_the_person_axis_unset() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input
        .constraints
        .push(max_days(vec![pb::CompactnessScope::Group as i32], 2));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert!(problem.constraints.max_days[0].group);
    assert!(!problem.constraints.max_days[0].person);
}

#[test]
fn max_consecutive_days_parses_the_same_way() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input
        .constraints
        .push(max_consecutive_days(vec![pb::CompactnessScope::Person as i32], 4));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.constraints.max_consecutive_days.len(), 1);
    assert!(!problem.constraints.max_consecutive_days[0].group);
    assert!(problem.constraints.max_consecutive_days[0].person);
    assert_eq!(problem.constraints.max_consecutive_days[0].max_consecutive_days, 4);
}

#[test]
fn weight_is_never_validated_hard_type() {
    // Unlike a SOFT type, a negative or NaN weight must not be refused —
    // HARD types ignore weight entirely, the same reason
    // `MaxConcurrentOnlineSessions` skips validation.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input
        .constraints
        .push(pb::ConstraintConfig { weight: -5.0, ..max_days(vec![], 1) });

    convert(&input, &scope(&["o1"])).expect("a HARD type's weight is never validated");
}
