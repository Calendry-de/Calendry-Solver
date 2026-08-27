//! The reverse direction: `(&Problem, &SolveOutcome) -> pb::SolverOutput`.
//!
//! Pure, ~80 lines, no branches worth hand-asserting one at a time, and it had
//! **zero** coverage — a textbook snapshot target. Every field a caller reads
//! comes out of this function: placed Session ids, slot references, room and
//! lecturer and group ids, the objective breakdown, and the stats.
//!
//! The snapshot pins the *shape* of the message, so a change to what the Nuxt
//! app receives cannot happen by accident. Determinism comes from a fixed seed
//! plus a **move** budget — a wall-clock-terminated run is legitimately not
//! reproducible, so a snapshot taken against one would be flaky by construction.
//! `elapsed_millis` is passed in as 0 for the same reason.

use calendry_solver::convert::{build_output, convert};
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, locked_session, offering, one_slot_grid, scope, slot};

const SEED: u64 = 0xC0FFEE;

/// A move budget, never a wall-clock budget.
fn budget() -> Budget {
    Budget { max_wall_millis: 0, max_moves: 20_000 }
}

fn output_for(input: &pb::SolverInput, in_scope: &[&str]) -> pb::SolverOutput {
    let problem = convert(input, &scope(in_scope)).expect("valid input");
    let outcome = solve(&problem, SEED, budget(), &NeverHalt);
    build_output(&problem, &outcome, 0)
}

#[test]
fn a_satisfiable_instance_renders_every_placed_session() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 3)];

    let output = output_for(&input, &["o1"]);
    insta::assert_debug_snapshot!(output);
}

#[test]
fn locked_sessions_are_not_echoed_as_placements() {
    // Only what this run placed comes back. The three locks are the caller's own
    // data; re-reporting them would double-count on the app side.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 5)];
    input.existing_sessions = vec![
        locked_session("s1", "o1", slot(0, 1, 1)),
        locked_session("s2", "o1", slot(0, 2, 1)),
        locked_session("s3", "o1", slot(0, 3, 1)),
    ];

    let output = output_for(&input, &["o1"]);
    assert_eq!(output.sessions.len(), 2, "5 required minus 3 locked");
    insta::assert_debug_snapshot!(output);
}

#[test]
fn an_infeasible_instance_renders_its_violations() {
    // One slot, two rooms, five Sessions required: three cannot be placed, and
    // the shortfall must reach the caller as an ExactFrequency violation rather
    // than as an error.
    let mut input = base_input();
    one_slot_grid(&mut input);
    input.offerings = vec![offering("o1", 5)];

    let output = output_for(&input, &["o1"]);
    assert!(!output.hard_violations.is_empty(), "a shortfall must be reported");
    insta::assert_debug_snapshot!(output);
}

#[test]
fn soft_components_appear_in_the_objective_breakdown() {
    // `ObjectiveBreakdown` shipped empty through slices 1-2 and now carries the
    // real weighted objective plus one component per configured soft instance.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 2)];
    let mut first = enabled(
        "c-first",
        pb::constraint_config::Params::MinimizeFirstBlock(pb::MinimizeFirstBlock {}),
    );
    first.weight = 3.0;
    let mut last = enabled(
        "c-last",
        pb::constraint_config::Params::MinimizeLastBlock(pb::MinimizeLastBlock {}),
    );
    last.weight = 2.0;
    input.constraints.push(first);
    input.constraints.push(last);

    let output = output_for(&input, &["o1"]);
    let objective = output
        .objective
        .as_ref()
        .expect("an objective is always reported");
    assert_eq!(objective.components.len(), 2, "one component per configured soft instance");
    insta::assert_debug_snapshot!(output);
}

#[test]
fn an_empty_scope_produces_an_output_with_nothing_placed() {
    // Not an error: a caller may legitimately ask for a run that has nothing to
    // do, and the stats and objective must still come back well-formed.
    let mut input = base_input();
    input.offerings = vec![offering("o1", 2)];

    let output = output_for(&input, &[]);
    assert!(output.sessions.is_empty());
    insta::assert_debug_snapshot!(output);
}
