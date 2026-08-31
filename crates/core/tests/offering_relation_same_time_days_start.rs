//! `SameTime` / `SameDays` / `SameStart` — the "parallel" counterpart to
//! `DifferentTime` on ADR-0028's relation mechanism (issue #54). All three
//! share one substrate (`constraints::member_week_sets`/`violated_weeks`):
//! per week, once 2+ members have a placed Session, their SETS of
//! `(day, block)` / `day` / `block` must be exactly equal.
//!
//! Unlike `DifferentTime`, none of the three is a construction filter — a
//! full day/block SET cannot be checked against a still-incomplete week
//! mid-search, so all three are HARD but PRICED (ADR-0025's stance,
//! `hard_penalty`-scale), read fresh off the solution the same way
//! `MaxDays`/`MaxConsecutiveDays` are.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::ids::{OfferingIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{Problem, ProblemSpec, RelationKind, RelationSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing::{
    fixture, grid, grid_5day, offering, room, structural_room_only,
};
use calendry_solver_core::{Placement, Solution};

mod common;
use common::{SEED, moves};

fn two_offerings_related(kind: RelationKind, blocks: u32, weeks: usize, day_count: u32) -> Problem {
    let grid = if day_count > 1 { grid_5day(blocks, weeks) } else { grid(blocks, weeks) };
    let a = offering("a", 1, &(0..blocks * day_count).collect::<Vec<_>>());
    let b = offering("b", 1, &(0..blocks * day_count).collect::<Vec<_>>());
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![a, b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid, structural_room_only())
    };
    spec.expand_placements();
    Problem::build(spec).unwrap()
}

fn place(problem: &Problem, a_start: SlotIdx, b_start: SlotIdx) -> Solution {
    let mut solution = Solution::empty(problem);
    solution.set(PlacementIdx(0), Some(Placement::single(a_start, RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(b_start, RoomIdx(1))));
    solution
}

// ---------------------------------------------------------------------------
// SameTime: (day, block) sets must match.
// ---------------------------------------------------------------------------

#[test]
fn same_time_reports_a_week_where_members_disagree_on_day_and_block() {
    // 2 blocks, one active day, one week: block 0 vs block 1 — same day,
    // different block, so the (day, block) sets disagree.
    let problem = two_offerings_related(RelationKind::SameTime, 2, 1, 1);
    let solution = place(&problem, SlotIdx(0), SlotIdx(1));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.same_time_violations, 1);
}

#[test]
fn same_time_is_not_reported_when_both_land_in_the_same_block() {
    let problem = two_offerings_related(RelationKind::SameTime, 2, 1, 1);
    let solution = place(&problem, SlotIdx(0), SlotIdx(0));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.same_time_violations, 0);
}

// ---------------------------------------------------------------------------
// SameDays: only the day-of-week set must match; block may differ.
// ---------------------------------------------------------------------------

#[test]
fn same_days_reports_a_week_where_members_use_different_days() {
    // 1 block/day, 5 active days, one week: Monday vs Tuesday.
    let problem = two_offerings_related(RelationKind::SameDays, 1, 1, 5);
    let solution = place(&problem, SlotIdx(0), SlotIdx(1));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.same_days_violations, 1);
}

#[test]
fn same_days_is_satisfied_by_a_shared_day_even_with_different_blocks() {
    // 2 blocks/day, one active day: both land on the SAME day but different
    // blocks — SameDays only compares the day-of-week set, so this must be
    // satisfied even though SameTime would not be.
    let problem = two_offerings_related(RelationKind::SameDays, 2, 1, 1);
    let solution = place(&problem, SlotIdx(0), SlotIdx(1));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.same_days_violations, 0);
    assert_eq!(objective.same_time_violations, 0, "SameDays is not configured on this relation");
}

// ---------------------------------------------------------------------------
// SameStart: only the block set must match; day-of-week may differ.
// ---------------------------------------------------------------------------

#[test]
fn same_start_reports_a_week_where_members_start_on_different_blocks() {
    let problem = two_offerings_related(RelationKind::SameStart, 2, 1, 1);
    let solution = place(&problem, SlotIdx(0), SlotIdx(1));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.same_start_violations, 1);
}

#[test]
fn same_start_is_satisfied_by_a_shared_block_even_on_different_days() {
    // 1 block/day, 5 active days: both start on block 0 — slot 0 is Monday,
    // slot 3 is Thursday — SameStart only compares the block set, so this
    // must be satisfied.
    let problem = two_offerings_related(RelationKind::SameStart, 1, 1, 5);
    let solution = place(&problem, SlotIdx(0), SlotIdx(3));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.same_start_violations, 0);
}

// ---------------------------------------------------------------------------
// Shared per-week-best-effort behavior, exercised once via SameTime.
// ---------------------------------------------------------------------------

#[test]
fn a_week_with_only_one_member_placed_imposes_nothing() {
    let problem = two_offerings_related(RelationKind::SameTime, 2, 1, 1);
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    // Offering "b" left entirely unplaced.

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.same_time_violations, 0);
}

#[test]
fn a_violation_in_one_week_does_not_leak_into_a_satisfied_week() {
    // 2 blocks/day, one active day, 2 weeks, 2 required Sessions per
    // Offering (one per week): week 0 disagrees, week 1 agrees.
    let a = offering("a", 2, &[0, 1]);
    let b = offering("b", 2, &[0, 1]);
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![a, b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::SameTime,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid(2, 2), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let mut solution = Solution::empty(&problem);
    // "a"'s two Sessions: week 0 block 0, week 1 block 0.
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(2), RoomIdx(0))));
    // "b"'s two Sessions: week 0 block 1 (disagrees), week 1 block 0 (agrees).
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(1), RoomIdx(1))));
    solution.set(PlacementIdx(3), Some(Placement::single(SlotIdx(2), RoomIdx(1))));

    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.same_time_violations, 1, "only week 0 disagrees");
}

#[test]
fn no_relation_configured_reports_nothing() {
    let a = offering("a", 1, &[0, 1]);
    let b = offering("b", 1, &[0, 1]);
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![a, b],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let solution = place(&problem, SlotIdx(0), SlotIdx(1));
    let objective = recompute_objective(&problem, &solution);
    assert_eq!(objective.same_time_violations, 0);
    assert_eq!(objective.same_days_violations, 0);
    assert_eq!(objective.same_start_violations, 0);
}

#[test]
fn evaluate_hard_reports_a_disagreeing_relation() {
    // Unlike `DifferentTime` (whose occupancy bitset also covers locked,
    // pre-placed input), `SameTime`/`SameDays`/`SameStart` read fresh off
    // the search's own movable placements only — there is no live occupancy
    // bit for a "full week's SET of days" to sit on. So the scenario worth
    // pinning here is the same one `recompute_objective` exercises: two
    // placed Sessions of related Offerings disagreeing within a week, this
    // time going through the reporting path.
    let problem = two_offerings_related(RelationKind::SameTime, 2, 1, 1);
    let solution = place(&problem, SlotIdx(0), SlotIdx(1));

    let violations = evaluate_hard(&problem, &solution);
    assert!(
        violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::SameTimeRelation
                && v.constraint_id == "rel-1"),
        "expected a SameTimeRelation violation, got {violations:?}"
    );
}

#[test]
fn a_disagreement_does_not_dead_end_construction() {
    // HARD but PRICED, per ADR-0025's stance: unlike DifferentTime, the
    // search must still place every Session even where satisfying the
    // relation is geometrically impossible (only 2 blocks, so 3 Offerings
    // sharing a relation and each needing its own room-pair can't all agree).
    let a = offering("a", 1, &[0, 1]);
    let b = offering("b", 1, &[0, 1]);
    let c = offering("c", 1, &[0, 1]);
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1"), room("r2")],
        offerings: vec![a, b, c],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::SameTime,
            members: vec![OfferingIdx(0), OfferingIdx(1), OfferingIdx(2)],
        }],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let outcome = solve(&problem, SEED, moves(1_000), &NeverHalt);
    assert_eq!(
        outcome.objective.unplaced, 0,
        "a HARD-but-priced relation must not refuse to place"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_same_relations() {
    for seed in 0..8u64 {
        let a = offering("a", 3, &[0, 1]);
        let b = offering("b", 3, &[0, 1]);
        let mut spec = ProblemSpec {
            rooms: vec![room("r0"), room("r1")],
            offerings: vec![a, b],
            relations: vec![
                RelationSpec {
                    id: "rel-time".into(),
                    kind: RelationKind::SameTime,
                    members: vec![OfferingIdx(0), OfferingIdx(1)],
                },
                RelationSpec {
                    id: "rel-days".into(),
                    kind: RelationKind::SameDays,
                    members: vec![OfferingIdx(0), OfferingIdx(1)],
                },
                RelationSpec {
                    id: "rel-start".into(),
                    kind: RelationKind::SameStart,
                    members: vec![OfferingIdx(0), OfferingIdx(1)],
                },
            ],
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
