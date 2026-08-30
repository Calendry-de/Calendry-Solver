//! The `(Offering, day)` cluster at the wire boundary:
//! `MaxOfferingSessionsPerDay`, `MaxConsecutiveOfferingBlocks` and
//! `MinimizeOfferingDaySplit` — no `CompactnessScope`, unlike their
//! Group/Person-axis siblings, since these are keyed by Offering alone.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

#[test]
fn max_offering_sessions_per_day_reaches_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(pb::ConstraintConfig {
        weight: 3.0,
        ..enabled(
            "c-off-count",
            pb::constraint_config::Params::MaxOfferingSessionsPerDay(
                pb::MaxOfferingSessionsPerDay { max_per_day: 2 },
            ),
        )
    });

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_offering_sessions_per_day_weight, 3.0);
    assert_eq!(problem.constraints.max_offering_sessions_per_day[0].max_per_day, 2);
}

#[test]
fn max_offering_sessions_per_day_rejects_a_negative_weight() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(pb::ConstraintConfig {
        weight: -1.0,
        ..enabled(
            "c-off-count",
            pb::constraint_config::Params::MaxOfferingSessionsPerDay(
                pb::MaxOfferingSessionsPerDay { max_per_day: 2 },
            ),
        )
    });

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}

#[test]
fn max_consecutive_offering_blocks_reaches_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(pb::ConstraintConfig {
        weight: 4.0,
        ..enabled(
            "c-off-run",
            pb::constraint_config::Params::MaxConsecutiveOfferingBlocks(
                pb::MaxConsecutiveOfferingBlocks { max_consecutive: 1 },
            ),
        )
    });

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_consecutive_offering_blocks_weight, 4.0);
    assert_eq!(problem.constraints.max_consecutive_offering_blocks[0].max_consecutive, 1);
}

#[test]
fn minimize_offering_day_split_reaches_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(pb::ConstraintConfig {
        weight: 2.0,
        ..enabled(
            "c-off-split",
            pb::constraint_config::Params::MinimizeOfferingDaySplit(
                pb::MinimizeOfferingDaySplit {},
            ),
        )
    });

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.minimize_offering_day_split_weight, 2.0);
    assert_eq!(problem.constraints.minimize_offering_day_split.len(), 1);
}

#[test]
fn minimize_offering_day_split_rejects_a_negative_weight() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 0)];
    input.constraints.push(pb::ConstraintConfig {
        weight: -1.0,
        ..enabled(
            "c-off-split",
            pb::constraint_config::Params::MinimizeOfferingDaySplit(
                pb::MinimizeOfferingDaySplit {},
            ),
        )
    });

    let e = convert(&input, &scope(&["o1"])).expect_err("negative weight must be refused");
    assert!(matches!(
        e,
        calendry_solver::error::ConvertError::NegativeSoftWeight { weight, .. } if weight == -1.0
    ));
}
