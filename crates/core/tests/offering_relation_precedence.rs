//! `Precedence` — a lab must follow its lecture (issue #37), the fourth
//! relation kind on ADR-0028's mechanism and the FIRST one that reads member
//! order.
//!
//! Three properties are worth pinning, because each one is a decision that
//! could plausibly have gone the other way and would fail silently if it
//! regressed:
//!
//! 1. **Term-wide, all pairs.** The boundary is the predecessor's LATEST end
//!    against the successor's EARLIEST start — not a per-week pairing (which
//!    the `SameTime` family uses) and not `UniTime`'s first-meetings-only.
//! 2. **Ordering is structural, the gap is wall-clock.** A fixture's default
//!    `GridTime` has `block_length_minutes == 0`, so every minute-of-day
//!    collapses to the day's start. Ordering must still be exact there.
//! 3. **HARD but PRICED**, like `SameTime`/`MaxDays`: an unsatisfiable
//!    relation must not dead-end construction.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::ids::{OfferingIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{Problem, ProblemSpec, RelationKind, RelationSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::slots::GridTime;
use calendry_solver_core::testing::{
    fixed_session, fixture, grid, grid_5day, offering, room, structural_room_only,
};
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn precedence(min_gap_minutes: u32, max_days_between: u32) -> RelationKind {
    RelationKind::Precedence { min_gap_minutes, min_days_between: 0, max_days_between }
}

/// The day FLOOR, with no minute gap and no ceiling — see
/// `day_counted_relations.rs` for why the floor cannot be spelled in minutes.
fn day_floor(min_days_between: u32) -> RelationKind {
    RelationKind::Precedence { min_gap_minutes: 0, min_days_between, max_days_between: 0 }
}

/// Two Offerings, `count` Sessions each, chained `a` before `b`.
fn chain(kind: RelationKind, count: u32, blocks: u32, weeks: usize, five_day: bool) -> Problem {
    let slots = if five_day { grid_5day(blocks, weeks) } else { grid(blocks, weeks) };
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![offering("a", count, &[0, 1]), offering("b", count, &[0, 1])],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(slots, structural_room_only())
    };
    spec.expand_placements();
    Problem::build(spec).unwrap()
}

/// `a`'s Sessions at `a_starts`, `b`'s at `b_starts`. Placement ids run
/// `a`'s occurrences first, then `b`'s.
fn place(problem: &Problem, a_starts: &[u32], b_starts: &[u32]) -> Solution {
    let mut solution = Solution::empty(problem);
    for (i, &s) in a_starts.iter().enumerate() {
        let p = PlacementIdx(i as u32);
        solution.set(p, Some(Placement::single(SlotIdx(s), RoomIdx(0))));
    }
    for (i, &s) in b_starts.iter().enumerate() {
        let p = PlacementIdx((a_starts.len() + i) as u32);
        solution.set(p, Some(Placement::single(SlotIdx(s), RoomIdx(1))));
    }
    solution
}

// ---------------------------------------------------------------------------
// The ordering itself. `grid(2, 1)` is 2 blocks on one day of one week, so
// slot 0 is block 0 and slot 1 is block 1.
// ---------------------------------------------------------------------------

#[test]
fn the_successor_starting_after_the_predecessor_is_satisfied() {
    let problem = chain(precedence(0, 0), 1, 2, 1, false);
    let solution = place(&problem, &[0], &[1]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

#[test]
fn the_successor_starting_before_the_predecessor_is_a_violation() {
    let problem = chain(precedence(0, 0), 1, 2, 1, false);
    let solution = place(&problem, &[1], &[0]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 1);
}

#[test]
fn sharing_one_block_is_a_violation_because_neither_precedes_the_other() {
    let problem = chain(precedence(0, 0), 1, 2, 1, false);
    let solution = place(&problem, &[0], &[0]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 1);
}

#[test]
fn member_order_is_what_decides_which_way_round_it_reads() {
    // The same two placements, with the relation's members reversed. Exactly
    // one of the two configurations may hold — this is the only kind that
    // reads `members`' order, so a bag would satisfy or fail both.
    let forward = chain(precedence(0, 0), 1, 2, 1, false);
    let placed = place(&forward, &[0], &[1]);
    assert_eq!(recompute_objective(&forward, &placed).precedence_violations, 0);

    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![offering("a", 1, &[0, 1]), offering("b", 1, &[0, 1])],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: precedence(0, 0),
            // b before a — the reverse of `chain`.
            members: vec![OfferingIdx(1), OfferingIdx(0)],
        }],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let reversed = Problem::build(spec).unwrap();
    let placed = place(&reversed, &[0], &[1]);
    assert_eq!(recompute_objective(&reversed, &placed).precedence_violations, 1);
}

// ---------------------------------------------------------------------------
// TERM-WIDE, ALL PAIRS — the decision that distinguishes this from a
// per-week pairing and from UniTime's first-meetings-only reading.
// ---------------------------------------------------------------------------

#[test]
fn every_session_of_the_predecessor_must_precede_every_session_of_the_successor() {
    // 2 blocks/day, one day, 2 weeks: slots 0,1 = week 0; slots 2,3 = week 1.
    // "a" in weeks 0 and 1, "b" in weeks 0 and 1 — interleaved, so each week
    // is individually ordered but "a"'s week-1 Session comes after "b"'s
    // week-0 one.
    let problem = chain(precedence(0, 0), 2, 2, 2, false);
    let solution = place(&problem, &[0, 2], &[1, 3]);

    assert_eq!(
        recompute_objective(&problem, &solution).precedence_violations,
        1,
        "a per-week pairing would call this satisfied; term-wide does not"
    );
}

#[test]
fn the_predecessor_finishing_entirely_before_the_successor_starts_is_satisfied() {
    // Both of "a"'s Sessions in week 0, both of "b"'s in week 1 — the
    // block-teaching shape this reading exists for.
    let problem = chain(precedence(0, 0), 2, 2, 2, false);
    let solution = place(&problem, &[0, 1], &[2, 3]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

#[test]
fn a_member_with_no_placed_session_imposes_nothing() {
    // Best-effort on the unplaced side, the answer issue #37 asked be stated.
    let problem = chain(precedence(0, 0), 1, 2, 1, false);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(0), RoomIdx(1))));
    // "a" left entirely unplaced, so there is no boundary to breach.

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

// ---------------------------------------------------------------------------
// LOCKED Sessions count. Unlike the `SameTime` family's placed-only scan —
// see the module section above `precedence_extents` for why the two differ.
// ---------------------------------------------------------------------------

#[test]
fn a_locked_session_of_the_predecessor_counts_toward_the_boundary() {
    // Every MOVABLE Session sits in order — "a" at block 0, "b" at block 1 —
    // so the boundary holds when only placements are counted. Adding a LOCKED
    // Session of "a" at block 1 moves "a"'s latest end level with "b"'s
    // start, and the boundary no longer holds. Asserted BOTH ways round, so
    // the test fails if locks stop counting: that is exactly the repair-mode
    // case where the predecessor is out of scope and the relation would
    // silently do nothing.
    let solve_with_lock = |locked: bool| {
        let mut spec = ProblemSpec {
            rooms: vec![room("r0"), room("r1")],
            offerings: vec![offering("a", 1, &[0, 1]), offering("b", 1, &[0, 1])],
            relations: vec![RelationSpec {
                id: "rel-1".to_string(),
                kind: precedence(0, 0),
                members: vec![OfferingIdx(0), OfferingIdx(1)],
            }],
            ..fixture(grid(2, 1), structural_room_only())
        };
        if locked {
            let mut f = fixed_session("a-locked", Some(0), 1);
            f.offering = Some(OfferingIdx(0));
            spec.fixed = vec![f];
        }
        spec.expand_placements();
        let problem = Problem::build(spec).unwrap();

        let mut solution = Solution::empty(&problem);
        for p in problem.placement_ids() {
            let slot = if problem.placement(p).offering == OfferingIdx(0) { 0 } else { 1 };
            solution.set(p, Some(Placement::single(SlotIdx(slot), RoomIdx(1))));
        }
        recompute_objective(&problem, &solution).precedence_violations
    };

    assert_eq!(solve_with_lock(false), 0, "the movable Sessions alone are in order");
    assert_eq!(solve_with_lock(true), 1, "the locked Session at block 1 is 'a''s latest end");
}

#[test]
fn a_locked_session_realizing_no_offering_is_no_relation_member() {
    // An ad-hoc Session (`offering: None`) can belong to no relation, so it
    // must not shift a boundary it has nothing to do with.
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![offering("a", 1, &[0, 1]), offering("b", 1, &[0, 1])],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: precedence(0, 0),
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        fixed: vec![fixed_session("ad-hoc", None, 1)],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let solution = place(&problem, &[0], &[1]);
    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

// ---------------------------------------------------------------------------
// A chain of three binds each CONSECUTIVE pair, not every pair.
// ---------------------------------------------------------------------------

#[test]
fn a_chain_of_three_binds_each_consecutive_pair() {
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1"), room("r2")],
        offerings: vec![
            offering("a", 1, &[0, 1, 2]),
            offering("b", 1, &[0, 1, 2]),
            offering("c", 1, &[0, 1, 2]),
        ],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: precedence(0, 0),
            members: vec![OfferingIdx(0), OfferingIdx(1), OfferingIdx(2)],
        }],
        ..fixture(grid(3, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let ordered = |a: u32, b: u32, c: u32| {
        let mut s = Solution::empty(&problem);
        s.set(PlacementIdx(0), Some(Placement::single(SlotIdx(a), RoomIdx(0))));
        s.set(PlacementIdx(1), Some(Placement::single(SlotIdx(b), RoomIdx(1))));
        s.set(PlacementIdx(2), Some(Placement::single(SlotIdx(c), RoomIdx(2))));
        recompute_objective(&problem, &s).precedence_violations
    };

    assert_eq!(ordered(0, 1, 2), 0, "a < b < c");
    assert_eq!(ordered(1, 0, 2), 1, "only the (a, b) boundary is wrong");
    assert_eq!(ordered(2, 1, 0), 2, "both boundaries are wrong");
}

// ---------------------------------------------------------------------------
// min_gap_minutes — wall-clock, resolved through GridTime, never blocks.
// ---------------------------------------------------------------------------

/// 60-minute blocks starting at 08:00, with a 30-minute break after block 0.
/// So block 0 is 08:00-09:00, block 1 is 09:30-10:30.
fn timed(kind: RelationKind, blocks: u32, weeks: usize) -> Problem {
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![offering("a", 1, &[0, 1]), offering("b", 1, &[0, 1])],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        grid_time: GridTime::new(60, 8 * 60, 0, vec![(0, 30, None)]),
        ..fixture(grid(blocks, weeks), structural_room_only())
    };
    spec.expand_placements();
    Problem::build(spec).unwrap()
}

#[test]
fn a_gap_shorter_than_required_is_a_violation() {
    // 30 minutes between block 0 ending and block 1 starting; 60 required.
    let problem = timed(precedence(60, 0), 2, 1);
    let solution = place(&problem, &[0], &[1]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 1);
}

#[test]
fn a_gap_exactly_meeting_the_requirement_is_satisfied() {
    let problem = timed(precedence(30, 0), 2, 1);
    let solution = place(&problem, &[0], &[1]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

#[test]
fn the_gap_is_measured_in_minutes_not_blocks() {
    // Three blocks: 08:00-09:00, 09:30-10:30, 10:30-11:30 (no break after
    // block 1). Block 0 -> block 2 is TWO blocks apart but only 90 minutes,
    // so a 120-minute requirement is breached even though a block-counting
    // implementation would see "two blocks" and pass anything up to two.
    let problem = timed(precedence(120, 0), 3, 1);
    let solution = place(&problem, &[0], &[2]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 1);
}

#[test]
fn ordering_holds_even_when_the_grid_carries_no_wall_clock_structure() {
    // The default `GridTime` has `block_length_minutes == 0`, so every block
    // on a day starts and ends at the same minute. Ordering is decided
    // structurally and must be exact anyway — the reason `Extreme` carries a
    // block ordinal alongside its minute.
    let problem = chain(precedence(0, 0), 1, 2, 1, false);
    assert_eq!(
        recompute_objective(&problem, &place(&problem, &[0], &[1])).precedence_violations,
        0
    );
    assert_eq!(
        recompute_objective(&problem, &place(&problem, &[1], &[0])).precedence_violations,
        1
    );
}

// ---------------------------------------------------------------------------
// max_days_between — CALENDAR days, so a weekend counts for three.
// ---------------------------------------------------------------------------

#[test]
fn zero_max_days_between_means_unbounded() {
    // 2 weeks apart, and no ceiling configured.
    let problem = chain(precedence(0, 0), 1, 2, 3, false);
    let solution = place(&problem, &[0], &[4]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

#[test]
fn a_boundary_further_apart_than_the_ceiling_is_a_violation() {
    // grid(2, 3) is one active day (Monday) per week, so slot 0 is week 0
    // Monday and slot 4 is week 2 Monday — 14 calendar days apart.
    let problem = chain(precedence(0, 7), 1, 2, 3, false);
    let solution = place(&problem, &[0], &[4]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 1);
}

#[test]
fn a_boundary_inside_the_ceiling_is_satisfied() {
    let problem = chain(precedence(0, 7), 1, 2, 3, false);
    // Slot 2 is week 1 Monday — 7 calendar days after slot 0.
    let solution = place(&problem, &[0], &[2]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

#[test]
fn the_ceiling_counts_calendar_days_not_teaching_days() {
    // `grid_5day` teaches Mon-Fri. Friday of week 0 is day index 4 (slots
    // 8-9 with 2 blocks/day); Monday of week 1 is day index 5 (slots 10-11).
    // One TEACHING day apart, three CALENDAR days apart — so a ceiling of 2
    // is breached and a ceiling of 3 is not.
    let two = chain(precedence(0, 2), 1, 2, 2, true);
    assert_eq!(
        recompute_objective(&two, &place(&two, &[8], &[10])).precedence_violations,
        1,
        "Friday -> Monday is 3 calendar days, above a ceiling of 2"
    );

    let three = chain(precedence(0, 3), 1, 2, 2, true);
    assert_eq!(recompute_objective(&three, &place(&three, &[8], &[10])).precedence_violations, 0);
}

// ---------------------------------------------------------------------------
// Reporting, and the invariant that the counter IS the reported count.
// ---------------------------------------------------------------------------

#[test]
fn evaluate_hard_reports_a_breached_boundary_naming_both_offerings() {
    let problem = chain(precedence(0, 0), 1, 2, 1, false);
    let solution = place(&problem, &[1], &[0]);

    let violations = evaluate_hard(&problem, &solution);
    let mine: Vec<_> = violations
        .iter()
        .filter(|v| v.constraint_type == ConstraintType::PrecedenceRelation)
        .collect();
    assert_eq!(mine.len(), 1, "expected one PrecedenceRelation violation, got {violations:?}");
    assert_eq!(mine[0].constraint_id, "rel-1");
    assert_eq!(mine[0].offering_ids, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn the_counter_equals_the_number_of_reported_violations() {
    // The two paths share one walk (`for_each_precedence_breach`) precisely
    // so they cannot disagree; this is the guard on that.
    let problem = chain(precedence(0, 0), 2, 2, 2, false);
    let solution = place(&problem, &[1, 3], &[0, 2]);

    let counted = recompute_objective(&problem, &solution).precedence_violations;
    let reported = evaluate_hard(&problem, &solution)
        .iter()
        .filter(|v| v.constraint_type == ConstraintType::PrecedenceRelation)
        .count();
    assert_eq!(counted as usize, reported);
    assert!(counted > 0, "the fixture is supposed to breach");
}

#[test]
fn an_out_of_order_boundary_is_reported_once_not_also_as_a_gap() {
    // `OutOfOrder` excludes the other two checks: a boundary in the wrong
    // order has no meaningful gap to be short or long.
    let problem = timed(precedence(600, 1), 2, 1);
    let solution = place(&problem, &[1], &[0]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 1);
}

#[test]
fn no_relation_configured_reports_nothing() {
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![offering("a", 1, &[0, 1]), offering("b", 1, &[0, 1])],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let solution = place(&problem, &[1], &[0]);
    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

// ---------------------------------------------------------------------------
// min_days_between — the FLOOR on the same boundary `max_days_between` caps.
//
// The family that would have been `NextDay`/`TwoDaysAfter` kinds. Why it is
// one scalar instead, and why no `min_gap_minutes` can stand in for it, is
// argued and pinned in `day_counted_relations.rs`; what follows mirrors this
// file's own ceiling and gap sections so the two bounds cannot drift.
// ---------------------------------------------------------------------------

#[test]
fn zero_min_days_between_is_todays_behaviour() {
    // proto3's scalar default, and therefore what every v0.17.0 peer sends.
    // A same-day ordered boundary must stay silent.
    let problem = chain(day_floor(0), 1, 2, 1, false);
    let solution = place(&problem, &[0], &[1]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

#[test]
fn a_boundary_closer_than_the_floor_is_a_violation() {
    // `grid_5day(2, 1)`: slots 0,1 are Monday, 2,3 are Tuesday.
    let problem = chain(day_floor(2), 1, 2, 1, true);
    let solution = place(&problem, &[0], &[2]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 1);
}

#[test]
fn a_boundary_exactly_on_the_floor_is_satisfied() {
    // `>=`, not `>` — the mirror of the ceiling's own inclusive bound.
    let problem = chain(day_floor(1), 1, 2, 1, true);
    let solution = place(&problem, &[0], &[2]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 0);
}

#[test]
fn the_floor_counts_calendar_days_not_teaching_days() {
    // The exact mirror of `the_ceiling_counts_calendar_days_not_teaching_days`,
    // so floor and ceiling cannot drift on the unit. Friday (day 4, slot 8) to
    // the next Monday (slot 10) is 3 CALENDAR days and 1 teaching day.
    let met = chain(day_floor(3), 1, 2, 2, true);
    assert_eq!(recompute_objective(&met, &place(&met, &[8], &[10])).precedence_violations, 0);

    let unmet = chain(day_floor(4), 1, 2, 2, true);
    assert_eq!(recompute_objective(&unmet, &place(&unmet, &[8], &[10])).precedence_violations, 1);
}

#[test]
fn a_day_floor_breach_is_reported_once_not_also_as_a_short_gap() {
    // The floor excludes the minute gap the way `OutOfOrder` excludes both: a
    // boundary on the wrong DAY has no meaningful minute gap to be short.
    // Without the exclusion one mistake would be charged TWICE at
    // `hard_penalty`, since `Objective::hard` sums this counter.
    //
    // Both bounds fail here: same-day placement, a floor of one day, and a
    // 10-hour minimum gap the boundary also misses.
    let problem = timed(
        RelationKind::Precedence { min_gap_minutes: 600, min_days_between: 1, max_days_between: 0 },
        2,
        1,
    );
    let solution = place(&problem, &[0], &[1]);

    assert_eq!(recompute_objective(&problem, &solution).precedence_violations, 1);
}

#[test]
fn a_contradictory_floor_and_ceiling_report_both() {
    // The floor does NOT suppress the ceiling, unlike the minute gap. Under
    // `min_days_between > max_days_between` the input is self-contradicting
    // and BOTH bounds are genuinely breached — the timetabler needs to see
    // both to understand why nothing can satisfy it. Tolerated and reported,
    // never refused, exactly as an over-long `min_gap_minutes` is.
    // Both fire only where `max < days < min`, which is exactly the
    // contradictory region: Monday to Wednesday is 2 days, below a floor of 5
    // and above a ceiling of 1.
    let problem = chain(
        RelationKind::Precedence { min_gap_minutes: 0, min_days_between: 5, max_days_between: 1 },
        1,
        2,
        1,
        true,
    );
    let solution = place(&problem, &[0], &[4]);

    assert_eq!(
        recompute_objective(&problem, &solution).precedence_violations,
        2,
        "2 days is below the floor of 5 and above the ceiling of 1 — both are real \
         findings, and suppressing either would hide half of why nothing fits"
    );
}

#[test]
fn the_counter_still_equals_the_number_of_reported_violations_with_a_floor() {
    // Extends the shared-walk invariant over the new breach variant.
    let problem = chain(day_floor(2), 2, 2, 2, true);
    let solution = place(&problem, &[0, 2], &[4, 6]);

    let counted = recompute_objective(&problem, &solution).precedence_violations;
    let reported = evaluate_hard(&problem, &solution)
        .iter()
        .filter(|v| v.constraint_type == ConstraintType::PrecedenceRelation)
        .count();
    assert_eq!(counted as usize, reported);
    assert!(counted > 0, "the fixture is supposed to breach");
}

#[test]
fn a_locked_predecessor_counts_toward_the_floor() {
    // The floor inherits ADR-0028's locks-count stance rather than quietly
    // diverging from it: `precedence_extents` reads `problem.fixed` too, and a
    // repair run locks every out-of-scope Session, so a placed-only scan would
    // make a relation with an out-of-scope predecessor silently inert.
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        // "a" needs no PLACEMENT: its one Session is the locked one below, so
        // the only placement variable in this fixture belongs to "b".
        offerings: vec![offering("a", 0, &[0, 1]), offering("b", 1, &[0, 1])],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: day_floor(2),
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid_5day(2, 2), structural_room_only())
    };
    // "a"'s Session is LOCKED on Tuesday (slot 2), not placed.
    let mut locked = fixed_session("a-locked", Some(0), 2);
    locked.offering = Some(OfferingIdx(0));
    spec.fixed = vec![locked];
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    // "b" on Wednesday (slot 4) is one calendar day after the LOCKED Tuesday
    // Session, below the floor of two.
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(4), RoomIdx(1))));

    assert_eq!(
        recompute_objective(&problem, &solution).precedence_violations,
        1,
        "the locked predecessor must be inside the boundary the floor measures"
    );
}

#[test]
fn an_unsatisfiable_day_floor_does_not_dead_end_construction() {
    // Still PRICED, not filtered: a floor no placement can satisfy must still
    // yield a full solution plus a reported breach (ADR-0025's stance).
    let problem = chain(day_floor(99), 1, 2, 1, true);

    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0, "a HARD-but-priced floor must not refuse to place");
    assert_eq!(outcome.objective.precedence_violations, 1, "and must report the breach");
}

// ---------------------------------------------------------------------------
// HARD but PRICED, and the drift guard.
// ---------------------------------------------------------------------------

#[test]
fn an_unsatisfiable_precedence_does_not_dead_end_construction() {
    // One block on one day: "b" cannot possibly start after "a" ends, so the
    // relation is unsatisfiable. ADR-0025's stance says the search must
    // still place both and report the breach.
    let problem = chain(precedence(0, 0), 1, 1, 1, false);

    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(
        outcome.objective.unplaced, 0,
        "a HARD-but-priced relation must not refuse to place"
    );
    assert_eq!(outcome.objective.precedence_violations, 1, "and must report the breach");
}

#[test]
fn incremental_objective_matches_full_recomputation_with_precedence() {
    for seed in 0..8u64 {
        let mut spec = ProblemSpec {
            rooms: vec![room("r0"), room("r1")],
            offerings: vec![offering("a", 3, &[0, 1]), offering("b", 3, &[0, 1])],
            relations: vec![RelationSpec {
                id: "rel-1".to_string(),
                kind: precedence(90, 10),
                members: vec![OfferingIdx(0), OfferingIdx(1)],
            }],
            grid_time: GridTime::new(60, 8 * 60, 15, vec![(1, 45, None)]),
            ..fixture(grid_5day(2, 3), structural_room_only())
        };
        spec.expand_placements();
        let problem = Problem::build(spec).unwrap();

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
