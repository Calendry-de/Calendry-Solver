//! `ExamSpacingSameDay` / `ExamSpacingWindow` at the wire boundary: both are
//! empty-or-near-empty messages, since which Sessions count as exam-kind is
//! `applies_to_kinds` on the enclosing `ConstraintConfig`, not a field here.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn same_day(weight: f64, kinds: Vec<String>) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        applies_to_kinds: kinds,
        ..enabled(
            "c-same-day",
            pb::constraint_config::Params::ExamSpacingSameDay(pb::ExamSpacingSameDay {}),
        )
    }
}

fn window(weight: f64, min_days_between: u32) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-window",
            pb::constraint_config::Params::ExamSpacingWindow(pb::ExamSpacingWindow {
                min_days_between,
            }),
        )
    }
}

#[test]
fn same_day_kinds_and_weight_reach_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(same_day(4.0, vec!["exam".into()]));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.exam_same_day_weight, 4.0);
    assert_eq!(problem.constraints.exam_spacing_same_day[0].kinds, vec!["exam".to_string()]);
}

#[test]
fn window_threshold_reaches_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(window(6.0, 3));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.exam_window_weight, 6.0);
    assert_eq!(problem.constraints.exam_spacing_window[0].min_days_between, 3);
}

#[test]
fn a_negative_weight_is_refused_for_either_type() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(same_day(-1.0, vec![]));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}
