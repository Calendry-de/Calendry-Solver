//! `MinimizeRoomChurn` at the wire boundary: `max_rooms_per_week` and
//! `applies_to_kinds` reaching `Problem`, plus the standard negative-weight
//! refusal every soft type gets.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn room_churn(weight: f64, max_rooms_per_week: u32, kinds: Vec<String>) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        applies_to_kinds: kinds,
        ..enabled(
            "c-churn",
            pb::constraint_config::Params::MinimizeRoomChurn(pb::MinimizeRoomChurn {
                max_rooms_per_week,
            }),
        )
    }
}

#[test]
fn max_rooms_per_week_and_kinds_reach_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input
        .constraints
        .push(room_churn(4.0, 2, vec!["lecture".into()]));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.room_churn_weight, 4.0);
    assert_eq!(problem.constraints.minimize_room_churn[0].max_rooms_per_week, 2);
    assert_eq!(problem.constraints.minimize_room_churn[0].kinds, vec!["lecture".to_string()]);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(room_churn(-1.0, 1, vec![]));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}
