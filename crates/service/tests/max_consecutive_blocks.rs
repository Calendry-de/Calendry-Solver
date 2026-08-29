//! `MaxConsecutiveBlocks` at the wire boundary: same `CompactnessScope`
//! resolution `Compactness` uses (empty = both axes), plus its own
//! `max_consecutive` threshold reaching `Problem`.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn max_consecutive(scope: Vec<i32>, weight: f64, max_consecutive: u32) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-run",
            pb::constraint_config::Params::MaxConsecutiveBlocks(pb::MaxConsecutiveBlocks {
                scope,
                max_consecutive,
            }),
        )
    }
}

#[test]
fn an_empty_scope_configures_both_axes() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(max_consecutive(vec![], 3.0, 4));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_consecutive_group_weight, 3.0);
    assert_eq!(problem.max_consecutive_person_weight, 3.0);
    assert_eq!(problem.constraints.max_consecutive_blocks[0].max_consecutive, 4);
}

#[test]
fn a_group_only_scope_leaves_the_person_axis_unweighted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input
        .constraints
        .push(max_consecutive(vec![pb::CompactnessScope::Group as i32], 3.0, 4));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_consecutive_group_weight, 3.0);
    assert_eq!(problem.max_consecutive_person_weight, 0.0);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(max_consecutive(vec![], -1.0, 4));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}
