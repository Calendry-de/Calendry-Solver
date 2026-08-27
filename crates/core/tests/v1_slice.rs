//! The v1 slice acceptance tests.
//!
//! These are written to be *falsifiable*, not merely green. In particular, test
//! 1 asserts an exact assignment rather than "no violations" — with nothing
//! forcing placement, an empty schedule satisfies room double-booking
//! vacuously, so a "no violations" assertion would pass on a solver that does
//! nothing at all.

use calendry_solver_core::constraints::ConstraintType;
use calendry_solver_core::ids::{PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{ProblemSpec, classify_immovable};
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_core::soft::SoftParams;
use calendry_solver_core::{Immovable, Placement, testing};

mod common;
use common::{SEED, solve_to_convergence as run};

// ---------------------------------------------------------------------------
// Test 1 — forced-unique packing
// ---------------------------------------------------------------------------

#[test]
fn forced_unique_packing_produces_the_one_feasible_assignment() {
    let problem = testing::forced_unique();
    let outcome = run(&problem);

    assert!(
        outcome.hard_violations.is_empty(),
        "expected a feasible solution, got {:?}",
        outcome.hard_violations
    );

    // Assert the EXACT assignment, not just feasibility.
    let expect = [
        ("A", RoomIdx(0), SlotIdx(0)),
        ("B", RoomIdx(1), SlotIdx(1)),
        ("C", RoomIdx(2), SlotIdx(2)),
    ];

    for (i, (offering_id, room, slot)) in expect.iter().enumerate() {
        let p = PlacementIdx(i as u32);
        assert_eq!(
            problem.offering_of(p).id,
            *offering_id,
            "placement {i} should belong to offering {offering_id}"
        );
        assert_eq!(
            outcome.solution.get(p),
            Some(Placement { start: *slot, room: *room }),
            "offering {offering_id} landed in the wrong cell"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2 — over-subscribed input degrades gracefully
// ---------------------------------------------------------------------------

#[test]
fn oversubscribed_input_terminates_and_reports_a_hard_violation() {
    // 4 required sessions, 3 room-slots available.
    let problem = testing::oversubscribed();
    let outcome = run(&problem);

    // It must place what it can rather than giving up or erroring.
    assert_eq!(outcome.solution.placed_count(), 3, "should fill all 3 available room-slots");

    // And it must SAY that it could not satisfy the demand.
    assert_eq!(outcome.hard_violations.len(), 1);
    let v = &outcome.hard_violations[0];
    assert_eq!(v.constraint_type, ConstraintType::ExactFrequency);
    assert_eq!(v.offering_ids, vec!["A".to_string()]);
    assert!(
        v.detail.contains("requires 4") && v.detail.contains("3 placed"),
        "violation should name the shortfall, got: {}",
        v.detail
    );

    // Crucially, it must NOT have resolved the shortfall by double-booking.
    assert!(
        !outcome
            .hard_violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::RoomDoubleBooking),
        "must not double-book to satisfy frequency"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — past Sessions are excluded from recalculation
// ---------------------------------------------------------------------------

#[test]
fn past_sessions_are_untouched_and_still_count_as_occupancy() {
    let problem = testing::immovable_blocks_first_slot(Immovable::Past);
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty());

    // Greedy construction wants slot 0; the past Session occupies it, so the
    // new Session must land at slot 1.
    assert_eq!(
        outcome.solution.get(PlacementIdx(0)),
        Some(Placement { start: SlotIdx(1), room: RoomIdx(0) }),
        "past session's slot must be treated as occupied"
    );

    // The past Session is still in the problem, unmoved.
    assert_eq!(problem.fixed.len(), 1);
    assert_eq!(problem.fixed[0].session_id, "pinned");
    assert_eq!(problem.fixed[0].start, SlotIdx(0));
    assert_eq!(problem.fixed[0].reason, Immovable::Past);
}

#[test]
fn past_classification_is_unconditional() {
    let reference = Some(SlotIdx(10));

    // Before the reference instant: past, regardless of lock or scope.
    assert_eq!(classify_immovable(SlotIdx(9), reference, false, true), Some(Immovable::Past));
    assert_eq!(classify_immovable(SlotIdx(9), reference, true, true), Some(Immovable::Past));

    // At or after the reference instant, in scope and unlocked: movable.
    assert_eq!(classify_immovable(SlotIdx(10), reference, false, true), None);

    // A reference past the end of the term makes everything past.
    assert_eq!(classify_immovable(SlotIdx(0), None, false, true), Some(Immovable::Past));
}

// ---------------------------------------------------------------------------
// Test 4 — locked Sessions, same behavior via a different code path
// ---------------------------------------------------------------------------

#[test]
fn locked_sessions_are_untouched_and_still_count_as_occupancy() {
    let problem = testing::immovable_blocks_first_slot(Immovable::Locked);
    let outcome = run(&problem);

    assert!(outcome.hard_violations.is_empty());
    assert_eq!(
        outcome.solution.get(PlacementIdx(0)),
        Some(Placement { start: SlotIdx(1), room: RoomIdx(0) }),
        "locked session's slot must be treated as occupied"
    );
    assert_eq!(problem.fixed[0].reason, Immovable::Locked);
}

#[test]
fn locked_and_out_of_scope_are_distinguished_from_day_one() {
    let reference = Some(SlotIdx(0));

    // Locked, in scope: absolute. Never relaxed, not even by v2.
    assert_eq!(classify_immovable(SlotIdx(5), reference, true, true), Some(Immovable::Locked));

    // Unlocked, out of scope: the ONLY variant v2's minimize-movement relaxes.
    assert_eq!(
        classify_immovable(SlotIdx(5), reference, false, false),
        Some(Immovable::OutOfScope)
    );

    // A lock outranks being out of scope, so v2 cannot relax it by accident.
    assert_eq!(classify_immovable(SlotIdx(5), reference, true, false), Some(Immovable::Locked));
}

#[test]
fn locked_and_past_reach_the_same_outcome_by_different_paths() {
    let locked = run(&testing::immovable_blocks_first_slot(Immovable::Locked));
    let past = run(&testing::immovable_blocks_first_slot(Immovable::Past));
    assert_eq!(locked.solution.get(PlacementIdx(0)), past.solution.get(PlacementIdx(0)));
}

// ---------------------------------------------------------------------------
// Test 5 — determinism
// ---------------------------------------------------------------------------

#[test]
fn same_input_and_seed_produce_identical_output() {
    let problem = testing::symmetric();

    let first = solve(&problem, SEED, Budget::default(), &NeverHalt);

    for attempt in 0..5 {
        let again = solve(&problem, SEED, Budget::default(), &NeverHalt);

        let a: Vec<_> = problem
            .placement_ids()
            .map(|p| first.solution.get(p))
            .collect();
        let b: Vec<_> = problem
            .placement_ids()
            .map(|p| again.solution.get(p))
            .collect();
        assert_eq!(a, b, "run {attempt} disagreed with the first run");

        assert_eq!(first.moves_evaluated, again.moves_evaluated);
        assert_eq!(first.moves_accepted, again.moves_accepted);
        assert_eq!(first.hard_violations, again.hard_violations);
        assert_eq!(first.termination_reason, again.termination_reason);
    }
}

#[test]
fn construction_is_seed_independent() {
    // Slice 3 moved the seed out of the constructive heuristic: construction is
    // now a pure function of the problem, and the seed influences only the LNS
    // phase. That is deliberate — a schedule is reproducible from the input
    // alone before any metaheuristic runs.
    //
    // `symmetric()` has no soft constraints, so its objective is already 0 and
    // LNS exits immediately with nothing to improve. Different seeds therefore
    // MUST agree here. The guard against determinism passing trivially now lives
    // in slice 3's `different_seeds_explore_differently`, which uses an instance
    // where the seed can actually matter.
    let problem = testing::symmetric();
    let a = solve(&problem, 1, Budget::default(), &NeverHalt);
    let b = solve(&problem, 999_983, Budget::default(), &NeverHalt);

    let pa: Vec<_> = problem.placement_ids().map(|p| a.solution.get(p)).collect();
    let pb: Vec<_> = problem.placement_ids().map(|p| b.solution.get(p)).collect();
    assert_eq!(pa, pb, "with nothing for LNS to do, the seed cannot matter");

    assert_eq!(a.iterations, 0, "no soft constraints means no LNS iterations");
    assert!(a.hard_violations.is_empty());
    assert!(b.hard_violations.is_empty());
}

// ---------------------------------------------------------------------------
// Budget plumbing
// ---------------------------------------------------------------------------

#[test]
fn move_budget_stops_the_run_early() {
    // The move budget is consumed by LNS candidate scoring, so it can only bind
    // on an instance LNS actually works on. Two properties are needed:
    //
    //  * soft constraints exist, so LNS runs at all, and
    //  * a zero-cost solution is UNREACHABLE, so the search cannot finish before
    //    the budget bites. `MinimizeRoomRank` at threshold 1 penalizes every
    //    room, making some cost unavoidable.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("S", 2, &[0])],
        constraints: testing::with_soft(vec![testing::soft(
            "rank",
            3.0,
            SoftParams::MinimizeRoomRank { rank_threshold: 1, invert: false },
        )]),
        ..ProblemSpec::new(testing::grid(4, 1))
    });

    let outcome = solve(&problem, SEED, Budget { max_wall_millis: 0, max_moves: 10 }, &NeverHalt);

    assert_eq!(outcome.termination_reason, "move_budget");
    // The budget is checked once per iteration, so a single batch may overshoot
    // it; what must hold is that the run stopped promptly rather than running to
    // the stagnation limit.
    assert!(outcome.moves_evaluated < 1_000, "got {}", outcome.moves_evaluated);
}
