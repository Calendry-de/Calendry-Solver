//! `DistributedPatternAdherence`/`BlockPatternAdherence`: the search must
//! actually steer an Offering toward its tagged pattern, not only report
//! adherence correctly — `aggregates::tests` already covers the counter
//! arithmetic in isolation.

use calendry_solver_core::aggregates::PatternAdherenceInstance;
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec, SchedulingPattern};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;

mod common;
use common::{SEED, moves};

fn pattern_rule(weight: f64) -> PatternAdherenceInstance {
    PatternAdherenceInstance { id: "c-pattern".into(), kinds: vec![], weight }
}

/// One Offering needing 3 Sessions, one Room, `weeks` weeks of `blocks`
/// blocks on the only active day — enough headroom that both the
/// one-consistent-slot arrangement (DISTRIBUTED) and the contiguous-window
/// arrangement (BLOCK) are always reachable.
fn one_offering(pattern: SchedulingPattern, weeks: usize, blocks: u32) -> ProblemSpec {
    let offering = testing::with_pattern(testing::offering("O", 3, &[0]), pattern);
    ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![offering],
        ..ProblemSpec::new(testing::grid(blocks, weeks))
    }
}

#[test]
fn distributed_pattern_converges_on_one_weekly_slot() {
    let spec = ProblemSpec {
        constraints: ConstraintSet {
            distributed_pattern_adherence: vec![pattern_rule(5.0)],
            ..testing::structural_room_only()
        },
        ..one_offering(SchedulingPattern::Distributed, 3, 2)
    };
    let problem = testing::assemble(spec);
    let outcome = solve(&problem, SEED, moves(20_000), &NeverHalt);

    assert_eq!(
        outcome.objective.unplaced, 0,
        "3 Sessions fit easily in 3 weeks x 2 blocks x 1 room"
    );
    assert_eq!(
        outcome.objective.scheduling_pattern_cost, 0.0,
        "one weekly slot repeated across all 3 weeks is always reachable here"
    );
}

#[test]
fn block_pattern_converges_on_a_contiguous_window() {
    let spec = ProblemSpec {
        constraints: ConstraintSet {
            block_pattern_adherence: vec![pattern_rule(5.0)],
            ..testing::structural_room_only()
        },
        ..one_offering(SchedulingPattern::Block, 5, 2)
    };
    let problem = testing::assemble(spec);
    let outcome = solve(&problem, SEED, moves(20_000), &NeverHalt);

    assert_eq!(
        outcome.objective.unplaced, 0,
        "3 Sessions fit easily in 5 weeks x 2 blocks x 1 room"
    );
    assert_eq!(
        outcome.objective.scheduling_pattern_cost, 0.0,
        "a 3-week contiguous window is always reachable in a 5-week term"
    );
}

#[test]
fn an_offering_tagged_for_one_pattern_is_untouched_by_the_other_instance() {
    // BLOCK-tagged Offering, but only DISTRIBUTED is configured: the instance
    // must price nothing, since `scheduling_pattern` does not match.
    let spec = ProblemSpec {
        constraints: ConstraintSet {
            distributed_pattern_adherence: vec![pattern_rule(5.0)],
            ..testing::structural_room_only()
        },
        ..one_offering(SchedulingPattern::Block, 5, 2)
    };
    let problem = testing::assemble(spec);
    assert_eq!(problem.distributed_pattern_weight, 5.0);
    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(
        outcome.objective.scheduling_pattern_cost, 0.0,
        "a BLOCK-tagged Offering must not be priced by a DISTRIBUTED instance"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_scheduling_pattern() {
    for seed in 0..8u64 {
        let spec = ProblemSpec {
            constraints: ConstraintSet {
                distributed_pattern_adherence: vec![pattern_rule(3.0)],
                block_pattern_adherence: vec![pattern_rule(5.0)],
                ..testing::structural_room_only()
            },
            offerings: vec![
                testing::with_pattern(
                    testing::offering("D", 3, &[0]),
                    SchedulingPattern::Distributed,
                ),
                testing::with_pattern(testing::offering("B", 3, &[0]), SchedulingPattern::Block),
            ],
            rooms: testing::rooms(2),
            ..ProblemSpec::new(testing::grid(2, 5))
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
