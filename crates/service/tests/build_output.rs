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
use calendry_solver_core::constraints::evaluate_hard;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx};
use calendry_solver_core::search::{Budget, NeverHalt, recompute_objective, solve};
use calendry_solver_core::{ConstraintType, Placement, Solution};
use calendry_solver_proto::v1 as pb;

mod common;
use common::{
    base_input, enabled, locked_session, offering, one_slot_grid, person, scope, session, slot,
};

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
    // Every Session here is invented by this run (no existing_sessions), so
    // session_id is empty per the wire contract — but placement_ref must still
    // carry a stable, distinct label for each, or a violation naming one could
    // not be told apart from another.
    let mut refs: Vec<&str> = output
        .sessions
        .iter()
        .map(|s| s.placement_ref.as_str())
        .collect();
    refs.sort_unstable();
    assert_eq!(refs, vec!["o1#0", "o1#1", "o1#2"]);
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
    // The 3 locks are FixedOccupancy, not reusable placement variables (see
    // `build_placements`), so both remaining placements are newly invented —
    // occurrence continues from where the locks left off in offering order.
    let mut refs: Vec<&str> = output
        .sessions
        .iter()
        .map(|s| s.placement_ref.as_str())
        .collect();
    refs.sort_unstable();
    assert_eq!(refs, vec!["o1#0", "o1#1"]);
    insta::assert_debug_snapshot!(output);
}

/// The gap this field exists to close: a hard violation whose `session_ids`
/// names a Session this run invented (`existing_session_id: None`, hence
/// `session_id: ""` on the wire) must still be resolvable to a concrete entry
/// in `sessions`. Before `placement_ref` existed, nothing in the output
/// matched the violation's label — the caller had a reference to nowhere.
///
/// Built by hand rather than through `solve()`: `LecturerVeto` is enforced as
/// a placement filter, so the search itself never produces this state (see
/// `calendry-solver-core`'s `a_blackout_violation_present_in_the_input_is_reported`,
/// where the same setup leaves the Session unplaced instead). `evaluate_hard`
/// is the authoritative, independent check and is exercised directly here —
/// the same way slice 4's own tests call it against a hand-built `Solution`.
#[test]
fn a_violation_naming_an_invented_session_resolves_to_a_concrete_placement_ref() {
    let mut input = base_input();
    one_slot_grid(&mut input);
    input.persons = vec![{
        let mut p = person("p1");
        // Empty axes = blacked out on every day/block/week, per the same
        // "empty means every value on that axis" convention as
        // `person_unavailability`.
        p.blackouts = vec![pb::Unavailability {
            days: vec![],
            blocks: vec![],
            weeks: vec![],
            reason: "test".into(),
        }];
        p
    }];
    input.offerings = vec![offering("o1", 1)];
    input
        .constraints
        .push(enabled("c-veto", pb::constraint_config::Params::LecturerVeto(pb::LecturerVeto {})));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    let only_slot = problem
        .slots
        .resolve(0, 1, 0)
        .expect("one_slot_grid's single slot must resolve");
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(only_slot, RoomIdx(0))));

    let hard_violations = evaluate_hard(&problem, &solution);
    let violation = hard_violations
        .iter()
        .find(|v| v.constraint_type == ConstraintType::LecturerVeto)
        .expect("placing into a full blackout must be reported");
    assert_eq!(violation.session_ids, vec!["o1#0".to_string()]);

    let outcome = calendry_solver_core::SolveOutcome {
        objective: recompute_objective(&problem, &solution),
        solution,
        hard_violations,
        moves_evaluated: 0,
        candidates_enumerated: 0,
        moves_accepted: 0,
        iterations: 0,
        termination_reason: "test",
    };
    let output = build_output(&problem, &outcome, 0);

    assert_eq!(output.sessions.len(), 1);
    assert_eq!(output.sessions[0].session_id, "", "invented: empty per the wire contract");
    assert_eq!(
        output.sessions[0].placement_ref, "o1#0",
        "must match the violation's session_ids entry exactly"
    );
    assert_eq!(output.hard_violations[0].session_ids, vec!["o1#0".to_string()]);
}

/// The other half of the contract: a Session that already existed and was
/// merely re-placed keeps its real id, and `placement_ref` must agree with
/// `session_id` rather than reverting to the synthetic form — a caller must
/// not see two different labels for the one Session depending which field it
/// reads.
#[test]
fn a_reused_session_carries_the_same_id_in_both_fields() {
    let mut input = base_input();
    input.offerings = vec![offering("o1", 1)];
    input.existing_sessions = vec![session("existing-1", "o1", slot(0, 1, 0))];

    let output = output_for(&input, &["o1"]);

    assert_eq!(output.sessions.len(), 1);
    assert_eq!(output.sessions[0].session_id, "existing-1");
    assert_eq!(output.sessions[0].placement_ref, "existing-1");
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
    // `MinimizeBlockUsage`, not the deprecated MinimizeFirstBlock /
    // MinimizeLastBlock pair: schema 0.7.0 supersedes both with one type
    // carrying `first` and `last` flags, and senders are asked to emit it.
    let mut first = enabled(
        "c-first",
        pb::constraint_config::Params::MinimizeBlockUsage(pb::MinimizeBlockUsage {
            blocks: vec![],
            first: true,
            last: false,
        }),
    );
    first.weight = 3.0;
    let mut last = enabled(
        "c-last",
        pb::constraint_config::Params::MinimizeBlockUsage(pb::MinimizeBlockUsage {
            blocks: vec![],
            first: false,
            last: true,
        }),
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
