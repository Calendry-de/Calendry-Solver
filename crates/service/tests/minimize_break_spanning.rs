//! `MinimizeBreakSpanning` at the wire boundary: the constraint instance
//! reaches core, and `TimeGrid.default_gap_minutes`/`breaks` reach
//! `Problem::grid_time` — the field the solver never carried before this
//! type (issue #26).

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn spanning(weight: f64) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-break",
            pb::constraint_config::Params::MinimizeBreakSpanning(pb::MinimizeBreakSpanning {}),
        )
    }
}

#[test]
fn the_instance_reaches_core() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(spanning(7.0));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.constraints.minimize_break_spanning.len(), 1);
    assert_eq!(problem.constraints.minimize_break_spanning[0].weight, 7.0);
}

#[test]
fn a_negative_weight_is_refused() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(spanning(-1.0));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(e, ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0));
}

#[test]
fn a_zero_weight_is_accepted() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(spanning(0.0));

    let problem = convert(&input, &scope(&["o1"])).expect("zero is a valid weight");
    assert_eq!(problem.constraints.minimize_break_spanning[0].weight, 0.0);
}

#[test]
fn timegrid_breaks_and_default_gap_reach_grid_time() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.time_grid = Some(pb::TimeGrid {
        default_gap_minutes: 10,
        breaks: vec![pb::TimeGridBreak {
            after_block_index: 2,
            duration_minutes: 45,
            label: "Lunch".into(),
            day_of_week: Some(1),
        }],
        ..input.time_grid.unwrap()
    });

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    // Monday (1): the named 45-minute override at position 2 wins over the
    // 10-minute default.
    assert_eq!(problem.grid_time.gap_after(2, 1), 45);
    // Any other position falls back to the default.
    assert_eq!(problem.grid_time.gap_after(0, 1), 10);
    // A day the override does not name also falls back to the default.
    assert_eq!(problem.grid_time.gap_after(2, 3), 10);
}

#[test]
fn no_breaks_configured_means_no_gaps_anywhere() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    // `base_input`'s grid already carries no breaks; assert it explicitly so
    // this test fails loudly if that fixture ever changes.
    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.grid_time.gap_after(0, 1), 0);
    assert_eq!(problem.grid_time.gap_minutes_within_span(1, 0, 2), 0);
}
