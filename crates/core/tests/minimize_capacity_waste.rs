//! `MinimizeCapacityWaste`: reward a good Room-size fit against
//! `Offering.min_capacity`, graded by RATIO rather than raw seat-count
//! distance.
//!
//! Not a `SoftParams` variant — see `problem::CapacityWasteInstance`'s own
//! doc for why (cost depends on THIS Offering's `min_capacity`, not only on
//! `(kind-profile, slot, room)`).

use calendry_solver_core::ids::RoomIdx;
use calendry_solver_core::problem::{CapacityWasteInstance, ConstraintSet, ProblemSpec, Room};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;

mod common;
use common::{SEED, moves};

fn capped(weight: f64, waste_ratio_threshold: f64) -> ConstraintSet {
    ConstraintSet {
        minimize_capacity_waste: vec![CapacityWasteInstance {
            id: "c-waste".into(),
            kinds: vec![],
            weight,
            waste_ratio_threshold,
        }],
        ..testing::structural_room_only()
    }
}

fn room_with_capacity(id: &str, capacity: u32) -> Room {
    Room { capacity, ..testing::room(id) }
}

#[test]
fn the_search_prefers_the_tighter_fitting_room() {
    // Room 0 fits closely (30 seats for 25 needed); Room 1 is needlessly huge
    // (200 seats). Both eligible, one slot, one Session.
    let offering = testing::with_min_capacity(testing::offering("O", 1, &[0, 1]), 25);
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_with_capacity("R0", 30), room_with_capacity("R1", 200)],
        offerings: vec![offering],
        constraints: capped(10.0, 1.0),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(2_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);

    let pl = outcome
        .solution
        .get(calendry_solver_core::ids::PlacementIdx(0))
        .unwrap();
    assert_eq!(pl.room, RoomIdx(0), "the tightly-fitting Room must win");
}

#[test]
fn a_zero_min_capacity_is_never_penalized() {
    let offering = testing::offering("O", 1, &[0]); // min_capacity defaults to 0
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_with_capacity("R0", 500)],
        offerings: vec![offering],
        constraints: capped(10.0, 1.0),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.soft, 0.0, "no min_capacity was ever stated");
}

#[test]
fn the_cost_formula_is_hand_computable() {
    let offering = testing::with_min_capacity(testing::offering("O", 1, &[0]), 20);
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_with_capacity("R0", 40)],
        offerings: vec![offering],
        constraints: capped(10.0, 1.0),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    // ratio = 40 / 20 = 2.0, excess over threshold 1.0 is 1.0,
    // cost = weight * (excess / (excess + 1)) = 10 * 0.5 = 5.0.
    let cost = problem.capacity_waste_cost(&problem.offerings[0], 40);
    assert_eq!(cost, 5.0);
}

#[test]
fn a_multi_room_placements_ratio_uses_summed_capacity() {
    let offering = testing::with_room_combinations(
        testing::with_min_capacity(testing::offering("O", 1, &[]), 40),
        2,
        &[0, 1],
    );
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![room_with_capacity("R0", 20), room_with_capacity("R1", 20)],
        offerings: vec![offering],
        constraints: capped(10.0, 1.0),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    // Summed capacity is 40, exactly min_capacity: ratio 1.0, at the
    // threshold, so excess is 0 and the cost is 0 — neither Room alone (20)
    // would even clear eligibility, so a per-Room ratio could never produce
    // this reading.
    let cost = problem.capacity_waste_cost(&problem.offerings[0], 40);
    assert_eq!(cost, 0.0);
}

#[test]
fn a_virtual_room_is_never_charged_for_capacity_waste() {
    // A virtual Room is not a scarce resource (ADR-0022): it has no seats to
    // waste. Capacity 999 against min_capacity 20 would be a ratio of ~50 for
    // a physical Room -- easily the biggest waste in the suite -- but must
    // cost nothing here.
    let offering = testing::with_min_capacity(testing::offering("O", 1, &[0]), 20);
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![Room { capacity: 999, ..testing::room_with("R0", 1, true) }],
        offerings: vec![offering],
        constraints: capped(10.0, 1.0),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(outcome.objective.soft, 0.0, "a virtual Room has nothing to waste");
}

#[test]
fn a_mixed_combination_charges_only_the_physical_rooms_capacity() {
    // Multi-room: one physical (20 seats), one virtual (999). The waste ratio
    // must read against 20 alone, not 1019 -- the virtual seat count is not
    // real scarcity to grade a fit against.
    let offering = testing::with_room_combinations(
        testing::with_min_capacity(testing::offering("O", 1, &[]), 20),
        2,
        &[0, 1],
    );
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![
            room_with_capacity("R0", 20),
            Room { capacity: 999, ..testing::room_with("R1", 1, true) },
        ],
        offerings: vec![offering],
        constraints: capped(10.0, 1.0),
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    // ratio = 20 / 20 = 1.0, at the threshold: excess 0, cost 0. Summing in
    // the virtual Room's 999 would instead give ratio ~51 and a large charge.
    let capacity = problem.exclusive_capacity([RoomIdx(0), RoomIdx(1)].into_iter());
    assert_eq!(problem.capacity_waste_cost(&problem.offerings[0], capacity), 0.0);
}

#[test]
fn incremental_objective_matches_full_recomputation_with_capacity_waste() {
    for seed in 0..8u64 {
        let spec = ProblemSpec {
            rooms: vec![
                room_with_capacity("R0", 20),
                room_with_capacity("R1", 60),
                room_with_capacity("R2", 150),
            ],
            offerings: vec![
                testing::with_min_capacity(testing::offering("A", 3, &[0, 1, 2]), 15),
                testing::with_min_capacity(testing::offering("B", 3, &[0, 1, 2]), 50),
            ],
            constraints: capped(4.0, 1.2),
            ..ProblemSpec::new(testing::grid(2, 3))
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
