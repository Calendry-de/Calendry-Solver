//! `MinimizeOfferingDistinctDays` at the wire boundary: `Offering.
//! prefer_fuller_days` reaches core alongside the instance, and the empty-
//! message conversion mirrors `DistributedPatternAdherence`/
//! `BlockPatternAdherence` exactly.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn distinct_days(weight: f64) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-days",
            pb::constraint_config::Params::MinimizeOfferingDistinctDays(
                pb::MinimizeOfferingDistinctDays {},
            ),
        )
    }
}

#[test]
fn the_instance_and_prefer_fuller_days_both_reach_core() {
    let mut input = base_input();
    input.offerings = vec![pb::Offering { prefer_fuller_days: true, ..offering("o1", 1) }];
    input.constraints.push(distinct_days(5.0));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.constraints.minimize_offering_distinct_days.len(), 1);
    assert_eq!(problem.constraints.minimize_offering_distinct_days[0].weight, 5.0);
    assert!(problem.offerings[0].prefer_fuller_days);
}

#[test]
fn the_flag_defaults_to_false() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert!(!problem.offerings[0].prefer_fuller_days);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(distinct_days(-1.0));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(e, ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0));
}

#[test]
fn a_zero_weight_is_accepted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(distinct_days(0.0));

    let problem = convert(&input, &scope(&["o1"])).expect("zero is a valid weight");
    assert_eq!(problem.constraints.minimize_offering_distinct_days[0].weight, 0.0);
}
