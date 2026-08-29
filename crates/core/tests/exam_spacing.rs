//! `ExamSpacingSameDay` / `ExamSpacingWindow`: a Group should not sit two
//! exam-kind Sessions too close together. Which Sessions count as exam-kind
//! is `applies_to_kinds` — the same kind-scoping mechanism every other type
//! uses, not a field of its own.
//!
//! Distinct from `MinimizeExamWeek`, which is about the exam period AS A
//! WHOLE; this is about exams that already fall inside it not landing too
//! close to each other.

use calendry_solver_core::aggregates::{ExamSpacingSameDayInstance, ExamSpacingWindowInstance};
use calendry_solver_core::problem::{ConstraintSet, ProblemSpec};
use calendry_solver_core::search::{NeverHalt, objectives_agree, recompute_objective, solve};
use calendry_solver_core::testing;

mod common;
use common::{SEED, moves};

const EXAM_KIND: &str = "exam";

fn exam_offering(id: &str, group: u32) -> calendry_solver_core::problem::OfferingSpec {
    calendry_solver_core::problem::OfferingSpec {
        kind: EXAM_KIND.into(),
        ..testing::with_groups(testing::offering(id, 1, &[0, 1]), &[group])
    }
}

fn same_day_rule(weight: f64) -> ExamSpacingSameDayInstance {
    ExamSpacingSameDayInstance {
        id: "c-exam-same-day".into(),
        kinds: vec![EXAM_KIND.into()],
        weight,
    }
}

fn window_rule(weight: f64, min_days_between: u32) -> ExamSpacingWindowInstance {
    ExamSpacingWindowInstance {
        id: "c-exam-window".into(),
        kinds: vec![EXAM_KIND.into()],
        weight,
        min_days_between,
    }
}

#[test]
fn the_search_spreads_two_exams_across_different_days() {
    // 3 days (1 block each), one Group, two exam-kind Offerings — plenty of
    // headroom to avoid the same day.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        groups: vec![testing::group("G", None)],
        offerings: vec![exam_offering("E0", 0), exam_offering("E1", 0)],
        constraints: ConstraintSet {
            exam_spacing_same_day: vec![same_day_rule(10.0)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(1, 3))
    });

    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.exam_same_day_cost, 0.0,
        "3 days for 2 exams always has a same-day-free arrangement"
    );
}

#[test]
fn a_non_exam_kind_sharing_a_day_is_not_counted() {
    // Same shape, but the second Offering is an ordinary "lecture" — not
    // covered by applies_to_kinds — sharing a day with the exam must cost
    // nothing.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        groups: vec![testing::group("G", None)],
        offerings: vec![
            exam_offering("E0", 0),
            testing::with_groups(testing::offering("L0", 1, &[0, 1]), &[0]),
        ],
        constraints: ConstraintSet {
            exam_spacing_same_day: vec![same_day_rule(10.0)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(
        outcome.objective.unplaced, 0,
        "one slot fits both only if neither blocks the other"
    );
    assert_eq!(outcome.objective.exam_same_day_cost, 0.0, "the lecture is not exam-kind");
}

#[test]
fn the_search_keeps_exams_at_least_min_days_apart() {
    // 5 days, one Group, two exam-kind Offerings, min_days_between = 3 —
    // headroom (days 0..4) always has a pair at least 3 apart (e.g. 0 and 3).
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        groups: vec![testing::group("G", None)],
        offerings: vec![exam_offering("E0", 0), exam_offering("E1", 0)],
        constraints: ConstraintSet {
            exam_spacing_window: vec![window_rule(10.0, 3)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(1, 5))
    });

    let outcome = solve(&problem, SEED, moves(5_000), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0);
    assert_eq!(
        outcome.objective.exam_window_cost, 0.0,
        "days 0 and 3 (or later) are always reachable"
    );
}

#[test]
fn incremental_objective_matches_full_recomputation_with_exam_spacing() {
    for seed in 0..8u64 {
        let offerings = vec![
            exam_offering("E0", 0),
            exam_offering("E1", 0),
            exam_offering("E2", 1),
            exam_offering("E3", 1),
        ];
        let spec = ProblemSpec {
            rooms: testing::rooms(2),
            groups: vec![testing::group("G0", None), testing::group("G1", None)],
            offerings,
            constraints: ConstraintSet {
                exam_spacing_same_day: vec![same_day_rule(4.0)],
                exam_spacing_window: vec![window_rule(3.0, 2)],
                ..testing::structural_room_only()
            },
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

#[test]
fn a_zero_weight_still_tracks_but_does_not_steer() {
    // One slot, two Rooms: both Offerings can only land on the same one day
    // that exists, whatever the weight.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        groups: vec![testing::group("G", None)],
        offerings: vec![exam_offering("E0", 0), exam_offering("E1", 0)],
        constraints: ConstraintSet {
            exam_spacing_same_day: vec![same_day_rule(0.0)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(1, 1))
    });

    let outcome = solve(&problem, SEED, moves(500), &NeverHalt);
    assert_eq!(outcome.objective.unplaced, 0, "one slot only, so both land on the same day");
    assert_eq!(outcome.objective.exam_same_day_cost, 0.0, "weight 0 charges nothing");
    let full = recompute_objective(&problem, &outcome.solution);
    assert_eq!(full.exam_same_day_cost, 0.0);
    // Direct check that the underlying tracking still saw the clash: reusing
    // the same shape with a nonzero weight must report it.
    let weighted = testing::assemble(ProblemSpec {
        rooms: testing::rooms(2),
        groups: vec![testing::group("G", None)],
        offerings: vec![exam_offering("E0", 0), exam_offering("E1", 0)],
        constraints: ConstraintSet {
            exam_spacing_same_day: vec![same_day_rule(7.0)],
            ..testing::structural_room_only()
        },
        ..ProblemSpec::new(testing::grid(1, 1))
    });
    let weighted_outcome = solve(&weighted, SEED, moves(500), &NeverHalt);
    assert_eq!(weighted_outcome.objective.exam_same_day_cost, 7.0);
}
