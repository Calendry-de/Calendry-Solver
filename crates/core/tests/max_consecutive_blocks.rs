//! `MaxConsecutiveBlocks`: the mirror image of `Compactness` — the search
//! must actively spread a run of back-to-back Sessions apart once it exceeds
//! the cap, not only report the excess correctly.

use calendry_solver_core::aggregates::MaxConsecutiveInstance;
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;

mod common;
use common::{SEED, moves};

fn run_rule(
    group: bool,
    person: bool,
    weight: f64,
    max_consecutive: u32,
) -> MaxConsecutiveInstance {
    MaxConsecutiveInstance {
        id: "c-run".into(),
        kinds: vec![],
        weight,
        group,
        person,
        max_consecutive,
    }
}

/// 6 blocks, one day, one Room, one Group, four single-block Sessions of
/// four separate Offerings all attached to that Group — enough headroom (6
/// blocks for 4 Sessions) that an arrangement with every run at or under 2
/// blocks is always reachable (e.g. blocks 0,1 and 3,4).
fn four_sessions_one_group(weight: f64, max_consecutive: u32) -> calendry_solver_core::Problem {
    let offerings = (0..4)
        .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 1, &[0]), &[0]))
        .collect();

    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![testing::group("G", None)],
        offerings,
        constraints: ConstraintSet {
            max_consecutive_blocks: vec![run_rule(true, false, weight, max_consecutive)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(6, 1))
    })
}

#[test]
fn the_search_breaks_up_a_long_run() {
    let problem = four_sessions_one_group(5.0, 2);
    let outcome = solve(&problem, SEED, moves(20_000), &NeverHalt);

    assert_eq!(outcome.objective.unplaced, 0, "all four must place: the day has room for them");
    assert_eq!(
        outcome.objective.max_consecutive_cost, 0.0,
        "4 Sessions in 6 blocks always has an arrangement with no run over 2; the search must find one"
    );
}

#[test]
fn a_zero_weight_still_tracks_the_count_it_just_does_not_steer_by_it() {
    let zero = four_sessions_one_group(0.0, 2);
    let outcome = solve(&zero, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.max_consecutive_cost, 0.0, "weight 0 charges nothing");

    // Same instance, weighted, to confirm the underlying tracking is live —
    // a run this long against a cap this tight cannot cost 0 by chance at
    // every seed with no pressure to avoid it.
    let weighted = four_sessions_one_group(5.0, 2);
    let unsteered = recompute_objective(
        &weighted,
        &calendry_solver_core::search::Trial::construct(&weighted)
            .solution()
            .clone(),
    );
    assert!(
        unsteered.max_consecutive_cost > 0.0 || unsteered.unplaced > 0,
        "greedy first-fit packs all four Sessions consecutively with nothing pushing them apart"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_max_consecutive_blocks() {
    for seed in 0..8u64 {
        let offerings = (0..5)
            .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 2, &[0, 1]), &[0]))
            .collect();
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            groups: vec![testing::group("G", None)],
            offerings,
            constraints: ConstraintSet {
                max_consecutive_blocks: vec![run_rule(true, true, 3.0, 2)],
                ..testing::structural_room_only()
            },
            ..ProblemSpec::new(testing::grid(3, 4))
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
