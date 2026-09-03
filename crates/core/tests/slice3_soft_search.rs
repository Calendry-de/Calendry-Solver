//! Slice 3 acceptance tests: LNS + SA, the six soft types, weighted objective.
//!
//! Slices 1 and 2 could assert exact assignments because their instances had a
//! single feasible packing. A metaheuristic makes tradeoffs, so these assert
//! **properties and directions** instead — chosen so each is red against a
//! plausible wrong implementation rather than merely green against the right
//! one.
//!
//! The two that carry the most weight:
//!
//! * the per-type direction tests (b), which fail against an evaluator that
//!   computes a number and steers nothing, and
//! * `incremental_objective_matches_full_recomputation` (f), which catches delta
//!   drift — a search silently optimizing a number that has diverged from the
//!   real objective. No other test here would notice that.

use calendry_solver_core::ids::PlacementIdx;
use calendry_solver_core::problem::ProblemSpec;
use calendry_solver_core::search::{
    NeverHalt, construct, objectives_agree, recompute_objective, solve,
};
use calendry_solver_core::soft::SoftParams;
use calendry_solver_core::{Problem, Solution, testing};

mod common;
use common::{SEED, moves, solve_with_move_budget as run};

fn placed(problem: &Problem, s: &Solution, i: u32) -> (u32, u32) {
    let p = s.get(PlacementIdx(i)).expect("should be placed");
    let _ = problem;
    (p.start.0, p.room.0)
}

// ---------------------------------------------------------------------------
// (a) Provably optimal micro-instance
// ---------------------------------------------------------------------------

#[test]
fn finds_the_hand_computable_optimum() {
    // 3 blocks, one room. First and last block each cost 4; block 1 costs 0.
    // The optimum is unique and computable by hand: slot 1, soft cost 0.
    let problem = testing::uniquely_optimal_middle_block();

    // Greedy alone takes block 0 — so the assertion below is about the
    // metaheuristic, not the constructive heuristic.
    let (greedy, _) = construct(&problem);
    assert_eq!(greedy.get(PlacementIdx(0)).unwrap().start.0, 0);
    assert_eq!(recompute_objective(&problem, &greedy).soft, 4.0);

    let outcome = run(&problem);
    assert_eq!(outcome.objective.soft, 0.0, "optimum is exactly 0");
    assert_eq!(placed(&problem, &outcome.solution, 0).0, 1, "block 1 is the only zero-cost slot");
    assert!(outcome.hard_violations.is_empty());
}

// ---------------------------------------------------------------------------
// (b) Per-type direction tests — one per soft type
//
// Each solves the same instance twice: once with the type at weight 0, once at
// a high weight, and asserts the schedule MOVES AWAY from the penalized
// feature. A decorative evaluator that reports a number but steers nothing
// fails every one of these.
// ---------------------------------------------------------------------------

fn slot_with_weight(build: impl Fn(f64) -> Problem) -> (u32, u32) {
    let off = run(&build(0.0));
    let on = run(&build(10.0));
    (
        off.solution.get(PlacementIdx(0)).unwrap().start.0,
        on.solution.get(PlacementIdx(0)).unwrap().start.0,
    )
}

fn room_with_weight(build: impl Fn(f64) -> Problem) -> (u32, u32) {
    let off = run(&build(0.0));
    let on = run(&build(10.0));
    (
        off.solution.get(PlacementIdx(0)).unwrap().room.0,
        on.solution.get(PlacementIdx(0)).unwrap().room.0,
    )
}

#[test]
fn minimize_first_block_steers_away_from_block_zero() {
    let (off, on) = slot_with_weight(|w| {
        testing::single_session(
            testing::grid(3, 1),
            testing::rooms(1),
            vec![testing::soft("f", w, SoftParams::MinimizeFirstBlock)],
        )
    });
    assert_eq!(off, 0, "unweighted, greedy takes the first block");
    assert_ne!(on, 0, "weighted, it must move off the first block");
}

#[test]
fn minimize_last_block_steers_away_from_the_final_block() {
    // A naive version of this test is ambiguous: with only the ends penalized
    // and two free middle blocks, several placements tie at cost 0 and the
    // assertion passes or fails on which tie the search happens to break.
    //
    // So the grid is narrowed to exactly two choices. Four blocks, one room,
    // with blocks 1 and 2 occupied by immovable Sessions — leaving block 0
    // (first, penalized 10) and block 3 (last, penalized `w`).
    //
    //   w = 0   -> block 3 costs 0, block 0 costs 10  => the LAST block is used
    //   w = 20  -> block 3 costs 20, block 0 costs 10  => it must move off it
    //
    // Against a no-op MinimizeLastBlock the weighted case still scores block 3
    // at 0 and stays there, so this fails rather than passing by luck.
    let build = |w: f64| {
        testing::assemble(ProblemSpec {
            rooms: testing::rooms(1),
            offerings: vec![testing::offering("S", 1, &[0])],
            fixed: vec![
                testing::fixed_session("blk1", Some(0), 1),
                testing::fixed_session("blk2", Some(0), 2),
            ],
            constraints: testing::with_soft(vec![
                testing::soft("f", 10.0, SoftParams::MinimizeFirstBlock),
                testing::soft("l", w, SoftParams::MinimizeLastBlock),
            ]),
            ..ProblemSpec::new(testing::grid(4, 1))
        })
    };

    let off = run(&build(0.0));
    assert_eq!(
        off.solution.get(PlacementIdx(0)).unwrap().start.0,
        3,
        "unweighted, the last block is the cheap option and must be taken"
    );

    let on = run(&build(20.0));
    assert_eq!(
        on.solution.get(PlacementIdx(0)).unwrap().start.0,
        0,
        "weighted above the first-block penalty, it must vacate the last block"
    );
}

#[test]
fn minimize_day_usage_steers_off_the_named_weekday() {
    // Monday (slot 0) and Saturday (slot 1). Penalize MONDAY, so the assertion
    // cannot pass by accident just because greedy prefers the earlier slot.
    let (off, on) = slot_with_weight(|w| {
        testing::single_session(
            testing::two_day_grid(),
            testing::rooms(1),
            vec![testing::soft(
                "d",
                w,
                SoftParams::MinimizeDayUsage { days: vec![1] },
            )],
        )
    });
    assert_eq!(off, 0, "unweighted, greedy takes Monday");
    assert_eq!(on, 1, "penalizing Monday must push it to Saturday");
}

#[test]
fn minimize_exam_week_steers_out_of_the_exam_week() {
    // Slot 0 is in the exam week, slot 1 is a teaching week.
    let (off, on) = slot_with_weight(|w| {
        testing::single_session_exam_week(testing::exam_week_grid(), testing::rooms(1), w, false)
    });
    assert_eq!(off, 0, "unweighted, greedy takes the earliest slot");
    assert_eq!(on, 1, "the exam week must be vacated");
}

#[test]
fn inverted_minimize_exam_week_steers_into_the_exam_week() {
    // Reversed grid (slot 0 teaching, slot 1 exam): greedy's unweighted
    // default lands OUTSIDE the exam week, so weighting the inverted rule has
    // to actively MOVE the Session in, not just fail to move it out. A bug
    // that dropped `invert` (behaving as if it were false, or ignored
    // entirely) would leave `on == 0` — it would never make the move.
    let (off, on) = slot_with_weight(|w| {
        testing::single_session_exam_week(
            testing::teaching_then_exam_grid(),
            testing::rooms(1),
            w,
            true,
        )
    });
    assert_eq!(off, 0, "unweighted, greedy takes the earliest slot");
    assert_eq!(on, 1, "inverted: the Session must move INTO the exam week");
}

#[test]
fn minimize_room_rank_steers_away_from_premium_rooms() {
    // R0 is rank 9 (premium) and comes first, so greedy grabs it.
    let (off, on) = room_with_weight(|w| {
        testing::single_session(
            testing::grid(1, 1),
            vec![
                testing::room_with("R0", 9, false),
                testing::room_with("R1", 1, false),
            ],
            vec![testing::soft(
                "r",
                w,
                SoftParams::MinimizeRoomRank { rank_threshold: 5, invert: false },
            )],
        )
    });
    assert_eq!(off, 0, "unweighted, greedy takes the premium room");
    assert_eq!(on, 1, "weighted, it must fall back to the ordinary room");
}

#[test]
fn minimize_online_steers_away_from_virtual_rooms() {
    let (off, on) = room_with_weight(|w| {
        testing::single_session(
            testing::grid(1, 1),
            vec![
                testing::room_with("R0", 1, true), // virtual
                testing::room_with("R1", 1, false),
            ],
            vec![testing::soft("o", w, SoftParams::MinimizeOnline)],
        )
    });
    assert_eq!(off, 0, "unweighted, greedy takes the online room");
    assert_eq!(on, 1, "weighted, it must move on-site");
}

// ---------------------------------------------------------------------------
// (c) Best-so-far never regresses
// (d) Never worse than the constructive start
// (e) Hard feasibility is not traded away
// ---------------------------------------------------------------------------

#[test]
fn search_never_returns_worse_than_the_greedy_start() {
    for seed in 0..8u64 {
        let problem = testing::seeded_instance(seed);
        let (greedy, _) = construct(&problem);
        let start = recompute_objective(&problem, &greedy);
        let outcome = solve(&problem, SEED, moves(50_000), &NeverHalt);

        assert!(
            outcome.objective.total(problem.hard_penalty)
                <= start.total(problem.hard_penalty) + 1e-9,
            "seed {seed}: search returned {:?}, worse than greedy {:?}",
            outcome.objective,
            start
        );
    }
}

#[test]
fn search_never_increases_hard_violations() {
    for seed in 0..8u64 {
        let problem = testing::seeded_instance(seed);
        let (greedy, _) = construct(&problem);
        let start = recompute_objective(&problem, &greedy);
        let outcome = solve(&problem, SEED, moves(50_000), &NeverHalt);

        assert!(
            outcome.objective.unplaced <= start.unplaced,
            "seed {seed}: feasibility regressed, {} unplaced vs {} at start",
            outcome.objective.unplaced,
            start.unplaced
        );
    }
}

#[test]
fn reported_objective_matches_the_returned_solution() {
    // Guards the "returned best is not the objective we reported" class of bug.
    for seed in 0..8u64 {
        let problem = testing::seeded_instance(seed);
        let outcome = solve(&problem, SEED, moves(50_000), &NeverHalt);
        let recomputed = recompute_objective(&problem, &outcome.solution);
        assert!(
            objectives_agree(outcome.objective, recomputed),
            "seed {seed}: reported {:?} but the solution scores {:?}",
            outcome.objective,
            recomputed
        );
    }
}

// ---------------------------------------------------------------------------
// (f) Incremental vs full objective — the delta-drift test
// ---------------------------------------------------------------------------

#[test]
fn incremental_objective_matches_full_recomputation() {
    // The search maintains the objective incrementally: ruin subtracts, repair
    // adds. If that diverges from a from-scratch computation, the search
    // optimizes a number that no longer describes the schedule — and every
    // other test here would still pass.
    //
    // Debug builds assert this on EVERY iteration inside the loop; this test
    // pins the end state across many instances and several budgets, so the
    // property holds at whatever point the run happens to stop.
    for seed in 0..12u64 {
        let problem = testing::seeded_instance(seed);

        for max_moves in [50u64, 500, 5_000, 50_000] {
            let outcome = solve(&problem, SEED ^ seed, moves(max_moves), &NeverHalt);
            let full = recompute_objective(&problem, &outcome.solution);

            assert_eq!(
                outcome.objective.unplaced, full.unplaced,
                "seed {seed} budget {max_moves}: unplaced count drifted"
            );
            assert!(
                (outcome.objective.soft - full.soft).abs() <= 1e-9 * (1.0 + full.soft.abs()),
                "seed {seed} budget {max_moves}: soft drifted, incremental {} vs full {}",
                outcome.objective.soft,
                full.soft
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Determinism under a move budget
// ---------------------------------------------------------------------------

#[test]
fn same_seed_and_move_budget_produce_identical_output() {
    for seed in 0..6u64 {
        let problem = testing::seeded_instance(seed);
        let first = solve(&problem, SEED, moves(50_000), &NeverHalt);

        for attempt in 0..3 {
            let again = solve(&problem, SEED, moves(50_000), &NeverHalt);

            let a: Vec<_> = problem
                .placement_ids()
                .map(|p| first.solution.get(p))
                .collect();
            let b: Vec<_> = problem
                .placement_ids()
                .map(|p| again.solution.get(p))
                .collect();
            assert_eq!(a, b, "seed {seed} attempt {attempt}: placements differ");

            assert_eq!(first.objective, again.objective);
            assert_eq!(first.moves_evaluated, again.moves_evaluated);
            assert_eq!(first.moves_accepted, again.moves_accepted);
            assert_eq!(first.iterations, again.iterations);
            assert_eq!(first.termination_reason, again.termination_reason);
            assert_eq!(first.hard_violations, again.hard_violations);
        }
    }
}

#[test]
fn different_seeds_explore_differently() {
    // Guards determinism passing trivially because the seed is ignored.
    let problem = testing::seeded_instance(3);
    let a = solve(&problem, 1, moves(50_000), &NeverHalt);
    let b = solve(&problem, 999_983, moves(50_000), &NeverHalt);
    assert_ne!(
        a.moves_evaluated, b.moves_evaluated,
        "different seeds should drive different search trajectories"
    );
}

#[test]
fn move_budget_is_respected() {
    let problem = testing::seeded_instance(5);
    let outcome = solve(&problem, SEED, moves(100), &NeverHalt);
    assert!(
        outcome.termination_reason == "move_budget" || outcome.termination_reason == "converged",
        "unexpected reason {}",
        outcome.termination_reason
    );
    if outcome.termination_reason == "move_budget" {
        // The budget is checked per iteration, so one batch may overshoot; what
        // must hold is that it stopped promptly rather than running to the end.
        assert!(outcome.moves_evaluated < 50_000);
    }
}

// ---------------------------------------------------------------------------
// Slices 1 and 2 must be unaffected by the metaheuristic
// ---------------------------------------------------------------------------

#[test]
fn instances_with_no_soft_constraints_converge_immediately() {
    // With nothing to optimize, the objective is already 0 and LNS must not
    // wander: the greedy result is returned untouched. This is what keeps the
    // slice 1 and 2 exact-assignment tests valid.
    let problem = testing::forced_unique();
    let outcome = run(&problem);
    assert_eq!(outcome.iterations, 0, "no soft constraints means nothing to do");
    assert_eq!(outcome.termination_reason, "converged");
    assert_eq!(outcome.objective.soft, 0.0);
}
