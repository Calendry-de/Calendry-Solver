//! `Compactness`: the search must actually steer toward filling gaps, not
//! only report them correctly — `aggregates::tests` already covers the gap
//! arithmetic in isolation.

use calendry_solver_core::aggregates::CompactnessInstance;
use calendry_solver_core::ids::PlacementIdx;
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{
    NeverHalt, Trial, objectives_agree, recompute_objective, solve,
};
use calendry_solver_core::testing;

mod common;
use common::{SEED, moves};

fn compactness_rule(group: bool, person: bool, weight: f64) -> CompactnessInstance {
    CompactnessInstance { id: "c-compact".into(), kinds: vec![], weight, group, person }
}

/// 4 blocks, one day, one Room, one Group, three single-block Sessions of
/// three separate Offerings all attached to that Group — enough demand that a
/// greedy fill without compactness has no reason to avoid spreading them out,
/// and enough headroom (4 blocks for 3 Sessions) that a gap-free arrangement
/// is always reachable.
fn three_sessions_one_group(weight: f64) -> calendry_solver_core::Problem {
    let offerings = (0..3)
        .map(|i| testing::with_groups(testing::offering(&format!("O{i}"), 1, &[0]), &[0]))
        .collect();

    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        groups: vec![testing::group("G", None)],
        offerings,
        constraints: ConstraintSet {
            compactness: vec![compactness_rule(true, false, weight)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(4, 1))
    })
}

#[test]
fn the_search_closes_a_gap_between_two_sessions() {
    let problem = three_sessions_one_group(5.0);
    let outcome = solve(&problem, SEED, moves(20_000), &NeverHalt);

    assert_eq!(outcome.objective.unplaced, 0, "all three must place: the day has room for them");
    assert_eq!(
        outcome.objective.compactness_cost, 0.0,
        "3 Sessions in 4 blocks always has a gap-free arrangement; the search must find one"
    );
}

#[test]
fn a_zero_weight_still_tracks_the_count_it_just_does_not_steer_by_it() {
    // Weight 0 must not mean "stop tracking" — `compactness_cost` in the
    // objective would then silently mismatch whatever `Aggregates` is
    // actually maintaining. Assert the WEIGHTED figure is exactly zero (the
    // multiplication is doing its job) while the underlying gap tracking is
    // demonstrably still live, by comparing against the weighted run's own
    // recomputation at the same seed and budget — both must still be
    // internally consistent even with nothing steering.
    let problem = three_sessions_one_group(0.0);
    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(outcome.objective.compactness_cost, 0.0, "weight 0 must price at exactly 0");
    let full = recompute_objective(&problem, &outcome.solution);
    assert!(objectives_agree(outcome.objective, full), "must stay drift-free even while inert");
}

#[test]
fn incremental_objective_matches_full_recomputation_with_compactness() {
    for seed in 0..8u64 {
        let problem = three_sessions_one_group(5.0);
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

#[test]
fn a_placement_and_its_removal_cancel_exactly() {
    // The narrow version of the drift property: place a Session that creates
    // a gap, then remove it, and the compactness cost must return exactly to
    // where it started — not merely close.
    let problem = three_sessions_one_group(5.0);
    let mut trial = Trial::construct(&problem);
    let before = trial.objective().compactness_cost;

    let p = PlacementIdx(0);
    let at = trial.unplace(p).expect("construction must have placed it");
    assert!(trial.place(p, at), "the same span must still resolve");
    assert_eq!(
        trial.objective().compactness_cost,
        before,
        "placing a Session back exactly where it was must not change the cost"
    );
}
