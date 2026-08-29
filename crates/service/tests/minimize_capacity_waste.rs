//! `MinimizeCapacityWaste` at the wire boundary: the waste ratio threshold
//! reaches core, `min_capacity` is threaded onto `Offering` (previously
//! discarded after eligibility filtering), and a negative weight is refused
//! like every other soft type.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn waste(weight: f64, threshold: f64) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-waste",
            pb::constraint_config::Params::MinimizeCapacityWaste(pb::MinimizeCapacityWaste {
                waste_ratio_threshold: threshold,
            }),
        )
    }
}

#[test]
fn the_instance_and_the_offerings_min_capacity_both_reach_core() {
    let mut input = base_input();
    input.offerings = vec![pb::Offering { min_capacity: 30, ..offering("o1", 1) }];
    input.constraints.push(waste(5.0, 1.5));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.constraints.minimize_capacity_waste.len(), 1);
    assert_eq!(problem.constraints.minimize_capacity_waste[0].waste_ratio_threshold, 1.5);
    assert_eq!(problem.offerings[0].min_capacity, 30);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(waste(-1.0, 1.0));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}

#[test]
fn a_zero_weight_is_accepted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(waste(0.0, 1.0));

    let problem = convert(&input, &scope(&["o1"])).expect("zero is a valid weight");
    assert_eq!(problem.constraints.minimize_capacity_waste[0].weight, 0.0);
}
