//! `MinimizeBreakSpanning`: discourage a Session's span from crossing a
//! `GridTime` gap — starting before a break and resuming after it.
//!
//! Not a `SoftParams` variant, for the same reason `MinimizeCapacityWaste`
//! is not: the cost depends on `Problem::grid_time`, not a
//! `(kind-profile, slot, room)` table. The STANDARD grid below mirrors the
//! calendry app's own `tests/timegrid-span-breaks.test.ts` fixture, so a
//! disagreement between the two implementations of the same walk would show
//! up here too.

use calendry_solver_core::ids::{PlacementIdx, SlotIdx};
use calendry_solver_core::problem::{
    ConstraintSet, MinimizeBreakSpanningInstance, OfferingSpec, ProblemSpec,
};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::slots::GridTime;
use calendry_solver_core::testing;

mod common;
use common::{SEED, moves};

/// 8 x 45min, breakMinutes: 0, three named universal breaks — after blocks
/// 0 (45m), 1 (15m) and 3 (30m).
fn standard_grid_time() -> GridTime {
    GridTime::new(45, 8 * 60, 0, vec![(0, 45, None), (1, 15, None), (3, 30, None)])
}

fn with_break_spanning(weight: f64) -> ConstraintSet {
    ConstraintSet {
        minimize_break_spanning: vec![MinimizeBreakSpanningInstance {
            id: "c-break".into(),
            kinds: vec![],
            weight,
        }],
        ..testing::structural_room_only()
    }
}

#[test]
fn the_cost_formula_charges_the_flat_weight_once_when_a_span_crosses_a_break() {
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![OfferingSpec { duration_blocks: 2, ..testing::offering("O", 1, &[0]) }],
        constraints: with_break_spanning(10.0),
        grid_time: standard_grid_time(),
        ..ProblemSpec::new(testing::grid(8, 1))
    });

    // Blocks 3-4 cross the break after block 3 (30 minutes) — charged the
    // instance's flat weight, not scaled by the 30 minutes.
    let crossing = problem.break_spanning_cost(&problem.offerings[0], SlotIdx(3), 2);
    assert_eq!(crossing, 10.0);

    // Blocks 4-5 are back to back: no override, default gap 0.
    let clean = problem.break_spanning_cost(&problem.offerings[0], SlotIdx(4), 2);
    assert_eq!(clean, 0.0);
}

#[test]
fn a_single_block_session_never_crosses_a_break() {
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![testing::offering("O", 1, &[0])],
        constraints: with_break_spanning(10.0),
        grid_time: standard_grid_time(),
        ..ProblemSpec::new(testing::grid(8, 1))
    });

    for block in 0..8u32 {
        assert_eq!(
            problem.break_spanning_cost(&problem.offerings[0], SlotIdx(block), 1),
            0.0,
            "block {block}: a span of 1 has no interior to cross"
        );
    }
}

#[test]
fn no_instance_configured_costs_nothing_even_across_a_named_break() {
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![OfferingSpec { duration_blocks: 2, ..testing::offering("O", 1, &[0]) }],
        constraints: testing::structural_room_only(), // no minimize_break_spanning
        grid_time: standard_grid_time(),
        ..ProblemSpec::new(testing::grid(8, 1))
    });

    assert_eq!(problem.break_spanning_cost(&problem.offerings[0], SlotIdx(3), 2), 0.0);
}

#[test]
fn a_kind_not_covered_by_the_instance_is_never_charged() {
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![OfferingSpec { duration_blocks: 2, ..testing::offering("O", 1, &[0]) }],
        constraints: ConstraintSet {
            minimize_break_spanning: vec![MinimizeBreakSpanningInstance {
                id: "c-break".into(),
                kinds: vec!["exam".into()], // this Offering's kind is "lecture"
                weight: 10.0,
            }],
            ..testing::structural_room_only()
        },
        grid_time: standard_grid_time(),
        ..ProblemSpec::new(testing::grid(8, 1))
    });

    assert_eq!(problem.break_spanning_cost(&problem.offerings[0], SlotIdx(3), 2), 0.0);
}

#[test]
fn the_search_prefers_the_gap_free_start() {
    // A duration-2 Offering with two otherwise-identical candidate starts:
    // blocks 3-4 (crosses the 30-minute break) and blocks 4-5 (back to
    // back). Nothing else distinguishes them, so the search must settle on
    // the gap-free one.
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room("R0")],
        offerings: vec![OfferingSpec { duration_blocks: 2, ..testing::offering("O", 1, &[0]) }],
        constraints: with_break_spanning(10.0),
        grid_time: standard_grid_time(),
        ..ProblemSpec::new(testing::grid(8, 1))
    });

    let outcome = solve(&problem, SEED, moves(2_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(outcome.objective.soft, 0.0, "a gap-free start exists and must win");

    let pl = outcome.solution.get(PlacementIdx(0)).unwrap();
    let f = problem.slots.flags(pl.start);
    assert_eq!(
        problem
            .grid_time
            .gap_minutes_within_span(f.iso_weekday, f.block, 2),
        0,
        "the chosen start must not cross a break"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_break_spanning() {
    for seed in 0..8u64 {
        let spec = ProblemSpec {
            rooms: vec![testing::room("R0"), testing::room("R1")],
            offerings: vec![
                OfferingSpec { duration_blocks: 2, ..testing::offering("A", 3, &[0, 1]) },
                OfferingSpec { duration_blocks: 3, ..testing::offering("B", 2, &[0, 1]) },
            ],
            constraints: with_break_spanning(6.0),
            grid_time: standard_grid_time(),
            ..ProblemSpec::new(testing::grid(8, 3))
        };
        let problem = testing::assemble(spec);
        let outcome = solve(&problem, SEED ^ seed, moves(500), &NeverHalt);
        let full = recompute_objective(&problem, &outcome.solution);
        assert!(
            objectives_agree(outcome.objective, full),
            "seed {seed}: drifted, incremental {:?} vs recomputed {:?}",
            outcome.objective,
            full
        );
    }
}
