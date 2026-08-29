//! `MinimizeLocationChange` at the wire boundary: same `CompactnessScope`
//! resolution `Compactness`/`MaxDailySpan` use, plus its own
//! `max_locations_per_day` threshold reaching `Problem`.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn location_change(
    scope: Vec<i32>,
    weight: f64,
    max_locations_per_day: u32,
) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-location",
            pb::constraint_config::Params::MinimizeLocationChange(pb::MinimizeLocationChange {
                scope,
                max_locations_per_day,
            }),
        )
    }
}

#[test]
fn an_empty_scope_configures_both_axes() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(location_change(vec![], 3.0, 2));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.location_change_group_weight, 3.0);
    assert_eq!(problem.location_change_person_weight, 3.0);
    assert_eq!(problem.constraints.minimize_location_change[0].max_locations_per_day, 2);
}

#[test]
fn a_person_only_scope_leaves_the_group_axis_unweighted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input
        .constraints
        .push(location_change(vec![pb::CompactnessScope::Person as i32], 3.0, 2));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.location_change_group_weight, 0.0);
    assert_eq!(problem.location_change_person_weight, 3.0);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(location_change(vec![], -1.0, 2));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}
