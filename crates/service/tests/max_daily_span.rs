//! `MaxDailySpan` at the wire boundary: same `CompactnessScope` resolution
//! `Compactness`/`MaxConsecutiveBlocks` use, plus its own `max_span_blocks`
//! threshold reaching `Problem`.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn max_daily_span(scope: Vec<i32>, weight: f64, max_span_blocks: u32) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-span",
            pb::constraint_config::Params::MaxDailySpan(pb::MaxDailySpan {
                scope,
                max_span_blocks,
            }),
        )
    }
}

#[test]
fn an_empty_scope_configures_both_axes() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(max_daily_span(vec![], 3.0, 4));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_daily_span_group_weight, 3.0);
    assert_eq!(problem.max_daily_span_person_weight, 3.0);
    assert_eq!(problem.constraints.max_daily_span[0].max_span_blocks, 4);
}

#[test]
fn a_person_only_scope_leaves_the_group_axis_unweighted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input
        .constraints
        .push(max_daily_span(vec![pb::CompactnessScope::Person as i32], 3.0, 4));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_daily_span_group_weight, 0.0);
    assert_eq!(problem.max_daily_span_person_weight, 3.0);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(max_daily_span(vec![], -1.0, 4));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}
