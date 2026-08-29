//! `MaxConcurrentOnlineSessions` at the wire boundary: the cap value comes
//! from the message field (`max_concurrent`), not `ConstraintConfig.weight`,
//! and `applies_to_kinds` is deliberately not read — the cap is tenant-wide.

use calendry_solver::convert::convert;
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

fn capped(cap: u32) -> pb::ConstraintConfig {
    enabled(
        "c-cap",
        pb::constraint_config::Params::MaxConcurrentOnlineSessions(
            pb::MaxConcurrentOnlineSessions { max_concurrent: cap },
        ),
    )
}

#[test]
fn the_cap_value_reaches_the_problem() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(capped(3));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_concurrent_online, Some(3));
}

#[test]
fn not_configured_means_no_cap() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_concurrent_online, None);
}

#[test]
fn two_instances_compose_as_the_tighter_cap() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(capped(5));
    input.constraints.push(capped(2));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");
    assert_eq!(problem.max_concurrent_online, Some(2), "the tightest cap governs");
}
