//! `RoomTurnaroundBuffer` at the wire boundary: `buffer_blocks` and
//! `applies_to_kinds` reaching `Problem`, plus the standard negative-weight
//! refusal every soft type gets.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn turnaround_buffer(weight: f64, buffer_blocks: u32, kinds: Vec<String>) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        applies_to_kinds: kinds,
        ..enabled(
            "c-buffer",
            pb::constraint_config::Params::RoomTurnaroundBuffer(pb::RoomTurnaroundBuffer {
                buffer_blocks,
            }),
        )
    }
}

#[test]
fn buffer_blocks_and_kinds_reach_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input
        .constraints
        .push(turnaround_buffer(4.0, 2, vec!["lab".into()]));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.room_turnaround_weight, 4.0);
    assert_eq!(problem.constraints.room_turnaround_buffer[0].buffer_blocks, 2);
    assert_eq!(problem.constraints.room_turnaround_buffer[0].kinds, vec!["lab".to_string()]);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(turnaround_buffer(-1.0, 1, vec![]));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}
