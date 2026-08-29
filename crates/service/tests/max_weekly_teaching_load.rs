//! `MaxWeeklyTeachingLoad` at the wire boundary: `count_blocks` and
//! `max_per_week` reach `Problem` unchanged, and a negative weight is
//! refused like every other soft type.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn load(weight: f64, count_blocks: bool, max_per_week: u32) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-load",
            pb::constraint_config::Params::MaxWeeklyTeachingLoad(pb::MaxWeeklyTeachingLoad {
                count_blocks,
                max_per_week,
            }),
        )
    }
}

#[test]
fn the_instance_reaches_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(load(3.0, true, 5));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_weekly_teaching_load_weight, 3.0);
    assert!(problem.constraints.max_weekly_teaching_load[0].count_blocks);
    assert_eq!(problem.constraints.max_weekly_teaching_load[0].max_per_week, 5);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(load(-1.0, false, 5));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}
