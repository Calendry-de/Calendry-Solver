//! The day-counted relation family: what `Precedence` already says, and the
//! one scalar it was genuinely missing (issue #55).
//!
//! `UniTime` carries ten separate distribution types on this axis — `1 Hour
//! Between` through `N Hours Between`, plus `Next Day` and `Two Days After` —
//! and the obvious reading of issue #55 is that Calendry needs the last two as
//! `RelationKind`s. It does not, and this file is the argument made executable,
//! on `mid_week_absence.rs`'s pattern: the cost of getting it wrong is a schema
//! change across three repos, so the reasoning should be checked by CI rather
//! than re-derived by the next reader.
//!
//! **"N hours between" was already exact.** `Precedence.min_gap_minutes` is a
//! wall-clock floor on the boundary, resolved through `GridTime` — block
//! lengths, the default gap, every named break. `UniTime` needs ten types only
//! because its distribution constraints are unparameterized enum values;
//! parametrizing the axis covers `90` and `75` too, which `UniTime` cannot say.
//!
//! **The day-counted floor was NOT expressible, and that is a proof rather
//! than a preference.** `min_gap_minutes: 1440` is the tempting workaround and
//! it is a wall-clock quantity answering a day-boundary question, so it
//! constrains time-of-day as a side effect. A threshold separating "same day"
//! from "next day" exists only inside a window derived from the grid — so it
//! moves when a break or a block is added — and on a teaching day spanning
//! twelve hours or more, **no value works at all**. Hence
//! `Precedence.min_days_between`.
//!
//! **But a scalar, not two kinds.** `NextDay` and `TwoDaysAfter` would be the
//! constants `1` and `2` welded into type names, on an axis whose other bound
//! is already a parameter (ADR-0024) — the `weeks[-exam_weeks:]` shape this
//! catalogue exists to replace. Two scalars also make the tenant *say* which
//! reading they mean: `min == max` is exact, `min` alone is at-least.
//!
//! Two readings stay refused, and the last two tests pin why: "the next
//! TEACHING day" is a unit this type does not measure in, and `UniTime`'s
//! per-occurrence pairing is a different pairing than this type declares.
//! Either would be a new kind, and nothing has asked for one.

use calendry_solver_core::ids::{OfferingIdx, PlacementIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{Problem, ProblemSpec, RelationKind, RelationSpec};
use calendry_solver_core::search::recompute_objective;
use calendry_solver_core::slots::GridTime;
use calendry_solver_core::testing::{
    fixture, grid, grid_5day, offering, room, structural_room_only,
};
use calendry_solver_core::{Placement, Solution};

mod common;

fn relation(min_gap_minutes: u32, min_days_between: u32, max_days_between: u32) -> RelationKind {
    RelationKind::Precedence { min_gap_minutes, min_days_between, max_days_between }
}

/// Two Offerings, one Session each, chained `a` before `b`, over a grid whose
/// wall-clock shape is given explicitly — which is the whole subject here.
fn chain(kind: RelationKind, blocks: u32, weeks: usize, five_day: bool, time: GridTime) -> Problem {
    let slots = if five_day { grid_5day(blocks, weeks) } else { grid(blocks, weeks) };
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![offering("a", 1, &[0, 1]), offering("b", 1, &[0, 1])],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        grid_time: time,
        ..fixture(slots, structural_room_only())
    };
    spec.expand_placements();
    Problem::build(spec).unwrap()
}

fn place(problem: &Problem, a: u32, b: u32) -> Solution {
    let mut solution = Solution::empty(problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(a), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(b), RoomIdx(1))));
    solution
}

fn breaches(problem: &Problem, solution: &Solution) -> u32 {
    recompute_objective(problem, solution).precedence_violations
}

/// 60-minute blocks from 08:00, one 30-minute break after block 0 — the same
/// shape `offering_relation_precedence.rs`'s own timed fixture uses. Block 0
/// is 08:00-09:00, block 1 09:30-10:30, block 2 10:30-11:30, block 3
/// 11:30-12:30.
fn short_day() -> GridTime {
    GridTime::new(60, 8 * 60, 0, vec![(0, 30, None)])
}

/// A teaching day spanning twelve hours: 60-minute blocks from 08:00, no
/// breaks. With 13 blocks the day runs 08:00-21:00 — an evening institution,
/// and the case where no wall-clock threshold can express a day floor.
fn long_day() -> GridTime {
    GridTime::new(60, 8 * 60, 0, vec![])
}

// ---------------------------------------------------------------------------
// "N hours between" — already expressible. Refuted, not built.
// ---------------------------------------------------------------------------

#[test]
fn n_hours_between_is_min_gap_minutes_and_nothing_else() {
    // THE FIRST REFUTATION. UniTime's whole hour-gap family is one existing
    // parameter. On `short_day`, block 0 ends 09:00 and block 2 starts 10:30:
    // a 90-minute boundary. So a 90-minute requirement is met and a
    // 120-minute one is not — and neither needed a new relation kind.
    let ninety = chain(relation(90, 0, 0), 4, 1, false, short_day());
    assert_eq!(breaches(&ninety, &place(&ninety, 0, 2)), 0, "90 minutes is exactly enough");

    let two_hours = chain(relation(120, 0, 0), 4, 1, false, short_day());
    assert_eq!(breaches(&two_hours, &place(&two_hours, 0, 2)), 1, "120 is not");

    // And the parametrized version says things UniTime cannot: 75 minutes.
    let seventy_five = chain(relation(75, 0, 0), 4, 1, false, short_day());
    assert_eq!(breaches(&seventy_five, &place(&seventy_five, 0, 2)), 0);
}

// ---------------------------------------------------------------------------
// The day floor — the one thing genuinely missing, and why
// ---------------------------------------------------------------------------

#[test]
fn no_wall_clock_floor_separates_same_day_from_next_day_on_a_long_day() {
    // THE IMPOSSIBILITY PROOF, and the whole justification for the new field.
    //
    // On a 13-block day running 08:00-21:00, compare the WIDEST same-day
    // boundary against the NARROWEST next-day one. If the same-day gap is at
    // least the next-day gap, then no single `min_gap_minutes` can admit every
    // next-day pair while rejecting every same-day pair: any threshold that
    // admits the next-day boundary also admits a same-day one.
    let blocks = 13;

    // Widest same-day boundary: block 0 ends 09:00, block 12 starts 20:00.
    let same_day_gap = 11 * 60;
    // Narrowest next-day boundary: block 12 ends 21:00 on day 1, block 0
    // starts 08:00 on day 2 — eleven hours later.
    let next_day_gap = 11 * 60;

    assert!(
        same_day_gap >= next_day_gap,
        "on a >=12h teaching day the widest same-day gap ({same_day_gap}) is not smaller \
         than the narrowest next-day gap ({next_day_gap}), so the two classes overlap"
    );

    // Executed rather than only computed: one threshold, both boundaries, and
    // it cannot tell them apart. `grid_5day` gives day 0 = Monday, so slot
    // arithmetic below is (day * blocks + block).
    let threshold = next_day_gap;
    let problem = chain(relation(threshold, 0, 0), blocks, 1, true, long_day());

    let widest_same_day = place(&problem, 0, blocks - 1);
    let narrowest_next_day = place(&problem, blocks - 1, blocks);
    assert_eq!(
        breaches(&problem, &widest_same_day),
        breaches(&problem, &narrowest_next_day),
        "a wall-clock threshold classifies a same-day and a next-day boundary \
         identically here, so it cannot express 'not before the next day'"
    );

    // The new scalar does tell them apart, on the same grid and the same two
    // placements. This is the field earning its existence.
    let floored = chain(relation(0, 1, 0), blocks, 1, true, long_day());
    assert_eq!(breaches(&floored, &place(&floored, 0, blocks - 1)), 1, "same day: breached");
    assert_eq!(breaches(&floored, &place(&floored, blocks - 1, blocks)), 0, "next day: satisfied");
}

#[test]
fn the_wall_clock_workaround_is_grid_dependent_even_when_it_exists() {
    // On a SHORT day a separating threshold does exist — and it is a property
    // of the grid, not of the rule. Widening the day by one block moves the
    // window, so a value that separated correctly yesterday silently stops
    // separating when a timetabler adds a block or a break.
    //
    // `short_day`, 4 blocks: widest same-day boundary is block 0 ending 09:00
    // to block 3 starting 11:30 = 150 minutes. So 151 separates.
    let four = chain(relation(151, 0, 0), 4, 1, true, short_day());
    assert_eq!(breaches(&four, &place(&four, 0, 3)), 1, "same day, correctly rejected");
    assert_eq!(breaches(&four, &place(&four, 3, 4)), 0, "next day, correctly admitted");

    // Six blocks: block 0 ends 09:00, block 5 starts 13:30 = 270 minutes, so
    // the same 151 now ADMITS a same-day boundary. The rule did not change;
    // the grid did.
    let six = chain(relation(151, 0, 0), 6, 1, true, short_day());
    assert_eq!(
        breaches(&six, &place(&six, 0, 5)),
        0,
        "the same threshold now admits a same-day boundary — expressible only by \
         accident of a particular grid"
    );

    // The day floor is immune: it is not measured in minutes at all.
    let floored = chain(relation(0, 1, 0), 6, 1, true, short_day());
    assert_eq!(breaches(&floored, &place(&floored, 0, 5)), 1, "still same day, still breached");
}

#[test]
fn next_day_is_precedence_with_a_floor_of_one_calendar_day() {
    // "Next Day" is a PARAMETER VALUE. `grid_5day(2, 1)` puts two blocks on
    // each of five days, so slots 0,1 are Monday and 2,3 are Tuesday.
    let problem = chain(relation(0, 1, 0), 2, 1, true, short_day());

    assert_eq!(breaches(&problem, &place(&problem, 0, 1)), 1, "same day is below the floor");
    assert_eq!(breaches(&problem, &place(&problem, 0, 2)), 0, "the next day meets it");
    assert_eq!(breaches(&problem, &place(&problem, 0, 4)), 0, "and so does later");
}

#[test]
fn two_days_after_is_the_same_scalar_set_to_two() {
    // The second constant is the same parameter. A separate `TwoDaysAfter`
    // kind would be `2` welded into a type name.
    let problem = chain(relation(0, 2, 0), 2, 1, true, short_day());

    assert_eq!(breaches(&problem, &place(&problem, 0, 2)), 1, "one day is below a floor of two");
    assert_eq!(breaches(&problem, &place(&problem, 0, 4)), 0, "two days meets it");
}

// ---------------------------------------------------------------------------
// The two readings that stay refused
// ---------------------------------------------------------------------------

#[test]
fn an_exact_day_distance_is_unsatisfiable_across_a_weekend() {
    // WHY "the next TEACHING day" is not offered. `min == max` is exact in
    // CALENDAR days, so a Friday predecessor demands a Saturday successor —
    // and `grid_5day` has no Saturday. Since `Precedence` is term-wide and
    // hard-priced, one Friday Session breaches the relation for the whole run,
    // permanently and silently.
    //
    // Friday is day 4, so with 2 blocks a day its slots are 8 and 9; the next
    // week's Monday is slot 10, three calendar days later.
    let problem = chain(relation(0, 1, 1), 2, 2, true, short_day());

    let across_the_weekend = place(&problem, 8, 10);
    assert_eq!(
        breaches(&problem, &across_the_weekend),
        1,
        "Friday to Monday is 3 calendar days: the floor of 1 is met, the ceiling of 1 \
         is not, and no legal successor slot satisfies both"
    );

    // Mid-week the same configuration is satisfiable, which is what makes the
    // Friday case a trap rather than an obvious error.
    assert_eq!(breaches(&problem, &place(&problem, 0, 2)), 0, "Monday to Tuesday is exactly 1");
}

#[test]
fn a_per_occurrence_pairing_is_not_what_precedence_measures() {
    // WHY UniTime's `Next Day` would be a NEW KIND, not a parameter. This
    // relation is term-wide and all-pairs: ONE boundary, the predecessor's
    // LAST end against the successor's FIRST start.
    //
    // So even a placement where every lab genuinely is the day after its own
    // lecture breaches a floor of one day — because lecture 2 (Wednesday) is
    // after lab 1 (Tuesday). Per-occurrence pairing is a different pairing
    // than this type declares, and ADR-0028 requires pairing be declared per
    // type.
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![offering("a", 2, &[0, 1]), offering("b", 2, &[0, 1])],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: relation(0, 1, 0),
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        grid_time: short_day(),
        ..fixture(grid_5day(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    // Lectures Monday (0) and Wednesday (4); labs Tuesday (2) and Thursday
    // (6). Each lab IS the day after its own lecture.
    let mut solution = Solution::empty(&problem);
    solution.set(PlacementIdx(0), Some(Placement::single(SlotIdx(0), RoomIdx(0))));
    solution.set(PlacementIdx(1), Some(Placement::single(SlotIdx(4), RoomIdx(0))));
    solution.set(PlacementIdx(2), Some(Placement::single(SlotIdx(2), RoomIdx(1))));
    solution.set(PlacementIdx(3), Some(Placement::single(SlotIdx(6), RoomIdx(1))));

    assert_eq!(
        breaches(&problem, &solution),
        1,
        "the boundary is the LAST lecture against the FIRST lab, so a per-occurrence \
         reading of this relation is not available as a parameter"
    );
}
