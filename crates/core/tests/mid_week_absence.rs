//! A mid-week absence is exactly expressible, and this is why (Calendry #118).
//!
//! That ticket reported an over-block — a person away Wednesday to Friday
//! loses the whole week of solver capacity — and diagnosed it as a wire-format
//! gap: "there is no way to say Monday and Tuesday of week 5 only in one row:
//! `{days: [1, 2], weeks: [5]}` also matches every OTHER week that has a
//! Monday/Tuesday". It proposed a date-precise axis on `Unavailability` in
//! `calendry-proto`, and a solver change to honour it.
//!
//! The over-block is real. The diagnosis is not, and it contradicts the
//! sentence above it in the same ticket: the three axes are CONJUNCTIVE, which
//! is exactly what makes `{days: [1, 2], weeks: [5]}` mean Monday and Tuesday
//! of week 5 and of nothing else. A date-precise axis would be a second way to
//! say what the format already says — the redundancy ADR-0028 exists to
//! prevent on the relation side — so the fix belongs entirely in the app's
//! `resolveHolidayWeeks`, which rounds a partial week up to a whole one before
//! the request is ever assembled.
//!
//! These tests exist so that reasoning is executable rather than filed in a
//! comment. They are cheap, and the cost of getting this wrong is not: the
//! next reader to reach for a fourth axis pays a schema change across three
//! repos for nothing.

use calendry_solver_core::Problem;
use calendry_solver_core::ids::SlotIdx;
use calendry_solver_core::problem::{ProblemSpec, Unavailability};
use calendry_solver_core::solution::{Occupant, SearchState};
use calendry_solver_core::testing::{self, blackout, person_with_blackouts, with_lecturers};

const MON: u32 = 1;
const TUE: u32 = 2;
const WED: u32 = 3;
const THU: u32 = 4;
const FRI: u32 = 5;

/// Three teaching weeks, Monday to Friday, one block a day — the smallest grid
/// on which "this Wednesday" and "every Wednesday" are different statements.
fn lecturer_away(windows: Vec<Unavailability>) -> Problem {
    testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![person_with_blackouts("P", &[], windows)],
        offerings: vec![with_lecturers(testing::offering("S", 1, &[0]), &[0])],
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::grid_5day(1, 3))
    })
}

/// Whether the veto actually blocks `(week, day)`, asked of the filter the
/// search itself consults rather than of a reimplementation of it.
fn blocked(problem: &Problem, week: u32, day: u32) -> bool {
    let state = SearchState::from_fixed(problem);
    let offering = &problem.offerings[0];
    let occupant = Occupant::of_offering(offering);
    let start: SlotIdx = problem
        .slots
        .lower_bound(week, day, 0)
        .expect("week/day in grid");
    let span = problem
        .slots
        .span(start, offering.duration_blocks)
        .expect("slot in grid");

    !state.is_free(problem, &occupant, &span)
}

/// The blocked cells of the three-week grid, as `(week, day)` pairs.
fn blocked_cells(problem: &Problem) -> Vec<(u32, u32)> {
    (0..3)
        .flat_map(|w| (1..=5).map(move |d| (w, d)))
        .filter(|&(w, d)| blocked(problem, w, d))
        .collect()
}

#[test]
fn one_row_says_wednesday_to_friday_of_one_week_and_nothing_else() {
    // THE CLAIM UNDER TEST, in its strongest form: the ticket's own example
    // absence, spelled in the axes that already exist, blocking exactly the
    // three days asked for.
    let problem = lecturer_away(vec![blackout(&[WED, THU, FRI], &[], &[1])]);

    assert_eq!(
        blocked_cells(&problem),
        vec![(1, WED), (1, THU), (1, FRI)],
        "the axes intersect, so the weekday set applies only within the week set",
    );
}

#[test]
fn the_conjunction_is_what_does_it_and_the_two_degenerate_rows_prove_that() {
    // The pair the ticket compared against, and the reason the row above is
    // not merely one lucky spelling. Dropping either axis widens the window in
    // a different direction — which is what a caller sees if they send half of
    // the pair, and is the actual shape of the app's bug.
    let every_week = lecturer_away(vec![blackout(&[WED, THU, FRI], &[], &[])]);
    assert_eq!(blocked_cells(&every_week).len(), 9, "no week axis: Wed-Fri of all three weeks",);

    let whole_week = lecturer_away(vec![blackout(&[], &[], &[1])]);
    assert_eq!(
        blocked_cells(&whole_week).len(),
        5,
        "no day axis: all five days of week 1 — the whole-week rounding #118 reports",
    );
}

#[test]
fn an_absence_crossing_a_week_boundary_is_two_rows() {
    // The one case a single cross product genuinely cannot cover: Wednesday of
    // week 1 through Tuesday of week 2. `blackouts` is a repeated field and
    // `Unavailability::matches` is applied with `any`, so the union of two
    // products says it exactly. Any set of cells is some union of products, so
    // the format is fully expressive — a date-precise axis would only ever
    // shorten a list, never enable a statement.
    let problem = lecturer_away(vec![
        blackout(&[WED, THU, FRI], &[], &[1]),
        blackout(&[MON, TUE], &[], &[2]),
    ]);

    assert_eq!(
        blocked_cells(&problem),
        vec![(1, WED), (1, THU), (1, FRI), (2, MON), (2, TUE)],
        "two rows, and the Monday and Tuesday of week 1 stay bookable",
    );
}

#[test]
fn blocks_narrow_a_row_to_part_of_a_day() {
    // The third axis intersects the same way, which is what makes "away
    // Wednesday afternoon of week 1" one row as well. Worth pinning alongside
    // the others: it is the case a date-precise axis would have had to carry
    // anyway, since a date names a day and not a time of day.
    let problem = testing::assemble(ProblemSpec {
        rooms: testing::rooms(1),
        persons: vec![person_with_blackouts(
            "P",
            &[],
            vec![blackout(&[WED], &[1], &[1])],
        )],
        offerings: vec![with_lecturers(testing::offering("S", 1, &[0]), &[0])],
        constraints: testing::all_constraints(),
        ..ProblemSpec::new(testing::grid_5day(2, 3))
    });

    let state = SearchState::from_fixed(&problem);
    let occupant = Occupant::of_offering(&problem.offerings[0]);
    let free_at = |block: u32| {
        let start = problem.slots.lower_bound(1, WED, block).expect("in grid");
        let span = problem.slots.span(start, 1).expect("in grid");
        state.is_free(&problem, &occupant, &span)
    };

    assert!(free_at(0), "the morning of the declared day stays bookable");
    assert!(!free_at(1));
}
