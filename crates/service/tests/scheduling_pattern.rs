//! `Offering.scheduling_pattern` and the two constraint types that read it —
//! `DistributedPatternAdherence` (`SCHEDULING_PATTERN_DISTRIBUTED`) and
//! `BlockPatternAdherence` (`SCHEDULING_PATTERN_BLOCK`). The cost arithmetic
//! itself is tested in `calendry_solver_core::aggregates`; these tests cover
//! the wire mapping — the enum resolving onto the right Offering, and an
//! instance only pricing the Offerings actually tagged for its own pattern.

use calendry_solver::convert::convert;
use calendry_solver_core::problem::SchedulingPattern;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn patterned(id: &str, pattern: pb::SchedulingPattern) -> pb::Offering {
    pb::Offering { scheduling_pattern: pattern as i32, ..offering(id, 0) }
}

fn distributed_adherence(weight: f64) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-distributed",
            pb::constraint_config::Params::DistributedPatternAdherence(
                pb::DistributedPatternAdherence {},
            ),
        )
    }
}

fn block_adherence(weight: f64) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-block",
            pb::constraint_config::Params::BlockPatternAdherence(pb::BlockPatternAdherence {}),
        )
    }
}

#[test]
fn the_wire_enum_resolves_onto_the_right_offering() {
    let mut input = base_input();
    input.offerings = vec![
        patterned("distributed", pb::SchedulingPattern::Distributed),
        patterned("block", pb::SchedulingPattern::Block),
        patterned("plain", pb::SchedulingPattern::Unspecified),
    ];

    let problem = convert(&input, &scope(&["distributed", "block", "plain"])).expect("valid input");
    assert_eq!(problem.offerings[0].scheduling_pattern, SchedulingPattern::Distributed);
    assert_eq!(problem.offerings[1].scheduling_pattern, SchedulingPattern::Block);
    assert_eq!(problem.offerings[2].scheduling_pattern, SchedulingPattern::Unspecified);
}

#[test]
fn an_unset_field_defaults_to_unspecified_not_an_error() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)]; // scheduling_pattern left at 0 (proto default)

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.offerings[0].scheduling_pattern, SchedulingPattern::Unspecified);
}

#[test]
fn distributed_pattern_adherence_configures_the_weight() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(distributed_adherence(4.0));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.distributed_pattern_weight, 4.0);
    assert_eq!(problem.block_pattern_weight, 0.0, "the other type must be untouched");
}

#[test]
fn block_pattern_adherence_configures_the_weight() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(block_adherence(6.0));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.block_pattern_weight, 6.0);
    assert_eq!(problem.distributed_pattern_weight, 0.0, "the other type must be untouched");
}

#[test]
fn a_negative_weight_is_refused_for_either_type() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(distributed_adherence(-1.0));

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}

#[test]
fn a_zero_weight_is_accepted_not_an_error() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(distributed_adherence(0.0));
    input.constraints.push(block_adherence(0.0));

    let problem = convert(&input, &scope(&["o1"])).expect("zero is a valid weight");
    assert_eq!(problem.distributed_pattern_weight, 0.0);
    assert_eq!(problem.block_pattern_weight, 0.0);
}
