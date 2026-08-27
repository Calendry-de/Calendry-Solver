//! Slice 4 acceptance tests: the three remaining hard types.
//!
//! These have three genuinely different shapes, and the tests are built to
//! prove each behaves according to its own shape rather than by accident:
//!
//! * `LecturerVeto` is **unary** — a slot mask, like the soft costs.
//! * `OnlineOnsiteSameDay` is a **day-granularity filter** the search can never
//!   violate.
//! * `MaxOnlineShare` is an **aggregate ratio** carried on the objective, so it
//!   *can* survive into a returned solution and be reported.
//!
//! The highest-value test here is `aggregate_counters_match_full_recomputation`
//! — the counterpart to slice 3's delta-drift test. The share counters have a
//! *moving denominator*, which is strictly more error-prone than the soft sums
//! were, so it is asserted per iteration in debug builds as well.

use calendry_solver_core::aggregates::ShareWindow;
use calendry_solver_core::constraints::{ViolationType, evaluate_hard};
use calendry_solver_core::ids::PlacementIdx;
use calendry_solver_core::problem::ProblemSpec;
use calendry_solver_core::search::{NeverHalt, recompute_objective, solve};
use calendry_solver_core::{Problem, testing};

mod common;
use common::{SEED, moves, solve_with_move_budget as run};

/// How many placed Sessions sit in a virtual room.
fn online_count(problem: &Problem, outcome: &calendry_solver_core::SolveOutcome) -> usize {
    problem
        .placement_ids()
        .filter_map(|p| outcome.solution.get(p))
        .filter(|pl| problem.rooms[pl.room.get()].is_virtual)
        .count()
}

// ---------------------------------------------------------------------------
// LecturerVeto — unary
// ---------------------------------------------------------------------------

#[test]
fn a_lecturer_is_not_scheduled_during_their_blackout() {
    // Two blocks, one room. The lecturer is blacked out on block 0, which is
    // exactly where greedy would otherwise put the Session.
    let problem = testing::lecturer_blacked_out_on_first_block(testing::all_constraints());
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(
        outcome.solution.get(PlacementIdx(0)).unwrap().start.0,
        1,
        "the blacked-out block must be avoided"
    );
}

#[test]
fn without_the_veto_the_same_instance_takes_the_blacked_out_slot() {
    // Falsification: with the rule disabled the blackout is inert and greedy
    // takes block 0. So the test above is measuring the rule, not the grid.
    let problem = testing::lecturer_blacked_out_on_first_block(testing::without_lecturer_veto());
    let outcome = run(&problem);
    assert_eq!(outcome.solution.get(PlacementIdx(0)).unwrap().start.0, 0);
}

#[test]
fn a_blackout_violation_present_in_the_input_is_reported() {
    // Force the clash: the lecturer is unavailable on every block, so no
    // placement can satisfy the veto and the search must leave it unplaced
    // rather than quietly scheduling into a blackout.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![testing::person_with_blackouts(
            "always-out",
            &[],
            vec![testing::blackout(&[], &[], &[])],
        )],
        offerings: vec![testing::with_lecturers(
            testing::offering("S", 1, &[0]),
            &[0],
        )],
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::grid(2, 1))
    });
    let outcome = run(&problem);

    assert_eq!(outcome.solution.placed_count(), 0, "nothing is placeable");
    assert!(
        !outcome
            .hard_violations
            .iter()
            .any(|v| v.constraint_type == ViolationType::LecturerVeto),
        "an unplaced Session must not also be reported as a veto breach"
    );
    assert!(outcome.objective.unplaced > 0, "the shortfall must surface on the objective");
}

// ---------------------------------------------------------------------------
// OnlineOnsiteSameDay — day-granularity filter
// ---------------------------------------------------------------------------

#[test]
fn a_group_does_not_mix_online_and_onsite_on_one_day() {
    let problem = testing::group_day_with_both_room_types(testing::all_constraints());
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty(), "{:?}", outcome.hard_violations);
    assert_eq!(outcome.solution.placed_count(), 2);

    let online = online_count(&problem, &outcome);
    assert!(
        online == 0 || online == 2,
        "the day must be all-online or all-on-site, got {online} of 2 online"
    );
}

#[test]
fn without_the_rule_the_same_instance_does_mix() {
    // Falsification: greedy reaches for the virtual room first, then the
    // on-site one, producing exactly the mix the rule exists to prevent.
    let problem = testing::group_day_with_both_room_types(testing::without_day_mix());
    let outcome = run(&problem);

    assert_eq!(outcome.solution.placed_count(), 2);
    assert_eq!(
        online_count(&problem, &outcome),
        1,
        "unconstrained, this instance mixes one online and one on-site"
    );
}

#[test]
fn a_mixed_day_already_in_immovable_input_is_reported() {
    // The search can never create a mix, so anything reported must have come
    // from the caller — which the "warn and allow" manual-edit UX permits.
    let mut online_fixed = testing::fixed_for_groups("pinned-online", 0, 0, &[0]);
    online_fixed.kind = "lecture".to_string();
    let mut onsite_fixed = testing::fixed_for_groups("pinned-onsite", 1, 1, &[0]);
    onsite_fixed.kind = "lecture".to_string();

    let problem = testing::assemble(ProblemSpec {
        rooms: testing::online_first_rooms(),
        groups: vec![testing::group("G", None)],
        fixed: vec![online_fixed, onsite_fixed],
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::grid(2, 1))
    });

    let violations = evaluate_hard(&problem, &calendry_solver_core::Solution::empty(&problem));
    assert!(
        violations
            .iter()
            .any(|v| v.constraint_type == ViolationType::OnlineOnsiteSameDay),
        "a pre-existing mixed day must be reported, got {violations:?}"
    );
}

// ---------------------------------------------------------------------------
// MaxOnlineShare — aggregate ratio
// ---------------------------------------------------------------------------

#[test]
fn the_online_share_cap_is_respected() {
    // 4 Sessions, cap 0.25 => floor(0.25 * 4) = 1 may be online.
    let problem =
        testing::share_capped_group(vec![testing::share_rule("cap", 0.25, ShareWindow::PerTerm)]);
    let outcome = run(&problem);

    assert_eq!(outcome.solution.placed_count(), 4);
    assert!(
        online_count(&problem, &outcome) <= 1,
        "cap allows 1 of 4 online, got {}",
        online_count(&problem, &outcome)
    );
    assert_eq!(outcome.objective.aggregate, 0, "no cell should be over");
}

#[test]
fn without_the_cap_more_sessions_go_online() {
    // Falsification: unconstrained, greedy fills the virtual room first, so
    // strictly more than the cap would allow ends up online.
    let problem = testing::share_capped_group(vec![]);
    let outcome = run(&problem);
    assert!(
        online_count(&problem, &outcome) > 1,
        "unconstrained this instance puts more than one online"
    );
}

#[test]
fn per_week_and_per_term_windows_differ() {
    // 4 Sessions over 2 weeks, cap 0.5.
    //   PER_TERM: 2 of 4 online is allowed.
    //   PER_WEEK: at most 1 of the 2 in each week.
    let term = run(&testing::share_across_two_weeks(vec![testing::share_rule(
        "cap",
        0.5,
        ShareWindow::PerTerm,
    )]));
    let week = run(&testing::share_across_two_weeks(vec![testing::share_rule(
        "cap",
        0.5,
        ShareWindow::PerWeek,
    )]));

    assert_eq!(term.objective.aggregate, 0);
    assert_eq!(week.objective.aggregate, 0);

    // The window is genuinely read: the per-week run must not concentrate its
    // online Sessions into a single week.
    let problem = testing::share_across_two_weeks(vec![testing::share_rule(
        "cap",
        0.5,
        ShareWindow::PerWeek,
    )]);
    let mut online_per_week = [0u32; 2];
    for p in problem.placement_ids() {
        if let Some(pl) = week.solution.get(p)
            && problem.rooms[pl.room.get()].is_virtual
        {
            online_per_week[problem.slots.flags(pl.start).week as usize] += 1;
        }
    }
    assert!(
        online_per_week.iter().all(|&n| n <= 1),
        "per-week cap allows 1 online per week, got {online_per_week:?}"
    );
}

#[test]
fn an_unsatisfiable_cap_is_reported_rather_than_silently_dropped() {
    // Only a virtual room exists, so every Session must be online, but the cap
    // is zero. MaxOnlineShare lives on the objective rather than acting as a
    // filter, so the run succeeds and REPORTS the breach — the same shape as an
    // unplaced Session, not a new exception.
    let problem = testing::assemble(ProblemSpec {
        rooms: vec![testing::room_with("V", 1, true)],
        groups: vec![testing::group("G", None)],
        offerings: vec![testing::with_groups(testing::offering("S", 2, &[0]), &[0])],
        constraints: calendry_solver_core::ConstraintSet {
            max_online_share: vec![testing::share_rule("cap", 0.0, ShareWindow::PerTerm)],
            ..testing::without_day_mix()
        },
        ..ProblemSpec::new(testing::grid(2, 1))
    });
    let outcome = run(&problem);

    assert_eq!(outcome.solution.placed_count(), 2, "placement still happens");
    assert!(outcome.objective.aggregate > 0, "the breach must reach the objective");
    assert!(
        outcome
            .hard_violations
            .iter()
            .any(|v| v.constraint_type == ViolationType::MaxOnlineShare),
        "and must be reported, got {:?}",
        outcome.hard_violations
    );
}

// ---------------------------------------------------------------------------
// The aggregate-counter drift test — highest value in this slice
// ---------------------------------------------------------------------------

#[test]
fn aggregate_counters_match_full_recomputation() {
    // The share counters are maintained incrementally across ruin and repair,
    // and unlike the soft sums their DENOMINATOR moves: relocating a Session
    // between weeks changes one numerator and two denominators. If those
    // counters drift, the search optimizes a constraint that no longer
    // describes the schedule, and every other test in this file still passes.
    //
    // Debug builds assert the whole objective — including this term — on every
    // LNS iteration; this pins the end state across many instances and budgets.
    for seed in 0..12u64 {
        let problem = testing::seeded_aggregate_instance(seed);

        for max_moves in [50u64, 500, 5_000, 50_000] {
            let outcome = solve(&problem, SEED ^ seed, moves(max_moves), &NeverHalt);
            let full = recompute_objective(&problem, &outcome.solution);

            assert_eq!(
                outcome.objective.aggregate, full.aggregate,
                "seed {seed} budget {max_moves}: share violations drifted \
                 (incremental {} vs recomputed {})",
                outcome.objective.aggregate, full.aggregate
            );
            assert_eq!(
                outcome.objective.unplaced, full.unplaced,
                "seed {seed} budget {max_moves}: unplaced drifted"
            );
            assert!(
                (outcome.objective.soft - full.soft).abs() <= 1e-9 * (1.0 + full.soft.abs()),
                "seed {seed} budget {max_moves}: soft drifted"
            );
        }
    }
}

#[test]
fn aggregate_instances_stay_deterministic() {
    for seed in 0..6u64 {
        let problem = testing::seeded_aggregate_instance(seed);
        let first = solve(&problem, SEED, moves(50_000), &NeverHalt);
        let again = solve(&problem, SEED, moves(50_000), &NeverHalt);

        let a: Vec<_> = problem
            .placement_ids()
            .map(|p| first.solution.get(p))
            .collect();
        let b: Vec<_> = problem
            .placement_ids()
            .map(|p| again.solution.get(p))
            .collect();
        assert_eq!(a, b, "seed {seed}");
        assert_eq!(first.objective, again.objective);
        assert_eq!(first.hard_violations, again.hard_violations);
    }
}

#[test]
fn the_search_never_creates_a_day_mix_or_a_veto_breach() {
    // Both are filters, so unlike MaxOnlineShare the search must never produce
    // them at all.
    for seed in 0..12u64 {
        let problem = testing::seeded_aggregate_instance(seed);
        let outcome = run(&problem);
        for v in &outcome.hard_violations {
            assert_ne!(
                v.constraint_type,
                ViolationType::OnlineOnsiteSameDay,
                "seed {seed}: filters must never be violated by the search"
            );
            assert_ne!(v.constraint_type, ViolationType::LecturerVeto, "seed {seed}");
        }
    }
}
