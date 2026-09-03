//! `Week.exam_group_ids` across the wire (Calendry #126): two cohorts sitting
//! their exams in different weeks.
//!
//! The pricing itself is covered in `crates/core/tests/minimize_exam_week.rs`.
//! What is checked here is the boundary, and specifically the two ways this
//! field could go wrong QUIETLY — both of which are refusals rather than
//! best-effort readings, because on this wire an empty scope means *every
//! Group*:
//!
//! * an unknown Group id, where dropping the only id would widen "cohort A's
//!   exam period" into the whole institution's;
//! * a scope on a week that is not an exam week, which could only ever be
//!   inert — and inert reads as "no exam period", putting ordinary teaching on
//!   top of the exams the scope was sent to protect.
//!
//! Plus the two readings that must NOT be refused: an empty list (the fail-open
//! convention, and what every peer on schema v0.17.0 or earlier sends), and a
//! scope naming a Group that resolves but that nothing attends.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_core::ids::SlotIdx;
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_proto::v1 as pb;
use tonic::Code;

mod common;
use common::{base_input, enabled, group, offering, scope};

const WEIGHT: f64 = 9.0;

/// A four-week calendar whose `kind`s and exam scopes are given per week.
fn calendar(weeks: &[(pb::WeekKind, Vec<&str>)]) -> pb::AcademicCalendar {
    pb::AcademicCalendar {
        term_id: "term-1".into(),
        weeks: weeks
            .iter()
            .enumerate()
            .map(|(i, (kind, groups))| pb::Week {
                index: i as u32,
                start_date: format!("2026-01-{:02}", 5 + i * 7),
                kind: *kind as i32,
                exam_group_ids: groups.iter().map(|g| (*g).to_string()).collect(),
            })
            .collect(),
        holidays: vec![],
    }
}

fn exam_week_rule(invert: bool) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight: WEIGHT,
        ..enabled(
            "c-exam-week",
            pb::constraint_config::Params::MinimizeExamWeek(pb::MinimizeExamWeek { invert }),
        )
    }
}

/// One Offering per cohort, each attached to its own Group.
fn per_cohort_offerings() -> (Vec<pb::Group>, Vec<pb::Offering>) {
    let groups = vec![group("g1"), group("g2")];
    let for_a = offering("for-a", 1);
    let for_b = pb::Offering { group_ids: vec!["g2".into()], ..offering("for-b", 1) };
    (groups, vec![for_a, for_b])
}

// ---------------------------------------------------------------------------
// The two refusals
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_group_in_exam_group_ids_is_refused() {
    // Dropping the id would NARROW the scope; dropping the only id would widen
    // it to every Group, turning one cohort's exam period into the whole
    // institution's. The same call every other id reference in `convert` makes.
    let mut input = base_input();
    input.calendar = Some(calendar(&[
        (pb::WeekKind::Exam, vec!["ghost"]),
        (pb::WeekKind::Teaching, vec![]),
    ]));
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(exam_week_rule(false));

    let error = convert(&input, &scope(&["o1"])).expect_err("this input must be refused");

    assert!(
        matches!(
            &error,
            ConvertError::UnknownGroup { context, group }
                if context == "calendar.weeks[0].exam_group_ids" && group == "ghost"
        ),
        "the context must name the field, so the app can find it: {error}",
    );
    assert_eq!(tonic::Status::from(error).code(), Code::InvalidArgument);
}

#[test]
fn exam_group_ids_on_a_teaching_week_is_refused() {
    // It could only ever be inert, and an inert scope reads as "no exam
    // period" — so lessons land on top of the exams while the run reports
    // nothing wrong. Structurally the same call `FootprintOnVirtualRoom`
    // makes: refuse the configuration whose only possible effect is silence.
    let mut input = base_input();
    input.calendar = Some(calendar(&[
        (pb::WeekKind::Teaching, vec![]),
        (pb::WeekKind::Teaching, vec!["g1"]),
    ]));
    input.offerings = vec![offering("o1", 1)];
    input.constraints.push(exam_week_rule(false));

    let error = convert(&input, &scope(&["o1"])).expect_err("this input must be refused");

    assert!(
        matches!(&error, ConvertError::ExamGroupsOnNonExamWeek { week, groups }
            if *week == 1 && groups == &vec!["g1".to_string()]),
        "unexpected error: {error}",
    );
    assert_eq!(
        tonic::Status::from(error).code(),
        Code::InvalidArgument,
        "bad input the caller can fix, not an unbuilt feature",
    );
}

// ---------------------------------------------------------------------------
// The readings that must NOT be refused
// ---------------------------------------------------------------------------

#[test]
fn an_empty_exam_group_ids_on_an_exam_week_binds_every_group() {
    // The fail-open convention AND the wire default in one assertion: an
    // absent `repeated string` decodes to empty, so this is exactly what every
    // peer on schema v0.17.0 or earlier sends, and it must keep meaning
    // "term-global exam period".
    let mut input = base_input();
    let (groups, offerings) = per_cohort_offerings();
    input.groups = groups;
    input.offerings = offerings;
    input.calendar = Some(calendar(&[
        (pb::WeekKind::Exam, vec![]),
        (pb::WeekKind::Teaching, vec![]),
    ]));
    input.constraints.push(exam_week_rule(false));

    let problem = convert(&input, &scope(&["for-a", "for-b"])).expect("valid input");

    for o in &problem.offerings {
        assert_eq!(
            problem.exam_week_cost(o, SlotIdx(0)),
            WEIGHT,
            "an unscoped exam week is every cohort's, including {}'s",
            o.id,
        );
    }
}

#[test]
fn a_week_scoped_to_a_group_nobody_attends_converts_and_is_inert() {
    // Distinguished from the unknown-id refusal: here the id RESOLVES, so
    // nothing widens — the scope simply matches nothing. Refusing would fail a
    // run over a cohort that happens to have no teaching yet, and the solver
    // tolerates inconsequential input.
    let mut input = base_input();
    input.groups = vec![group("g1"), group("g2")];
    input.offerings = vec![offering("o1", 1)]; // attached to g1 only
    input.calendar = Some(calendar(&[
        (pb::WeekKind::Exam, vec!["g2"]),
        (pb::WeekKind::Teaching, vec![]),
    ]));
    input.constraints.push(exam_week_rule(false));

    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    let o = &problem.offerings[0];
    assert_eq!(problem.exam_week_cost(o, SlotIdx(0)), 0.0, "not this cohort's exam period");
}

// ---------------------------------------------------------------------------
// End to end
// ---------------------------------------------------------------------------

#[test]
fn a_scoped_exam_period_survives_the_wire() {
    // The whole feature, from a `SolverInput` to placements: two cohorts, two
    // different exam weeks, an inverted rule so each cohort's exam Session
    // SEEKS its own period. Before `exam_group_ids` existed there was one
    // institution-wide answer and one of these two Sessions had to be wrong.
    let mut input = base_input();
    let (groups, offerings) = per_cohort_offerings();
    input.groups = groups;
    input.offerings = offerings;
    input.calendar = Some(calendar(&[
        (pb::WeekKind::Exam, vec!["g1"]),
        (pb::WeekKind::Exam, vec!["g2"]),
        (pb::WeekKind::Teaching, vec![]),
        (pb::WeekKind::Teaching, vec![]),
    ]));
    input.constraints.push(exam_week_rule(true));

    let problem = convert(&input, &scope(&["for-a", "for-b"])).expect("valid input");
    // A move budget, never wall-clock: a time-boxed run is not reproducible.
    let outcome =
        solve(&problem, 0xC0FFEE, Budget { max_wall_millis: 0, max_moves: 5_000 }, &NeverHalt);

    let week_of = |i: usize| {
        let p = outcome
            .solution
            .get(calendry_solver_core::ids::PlacementIdx(i as u32))
            .expect("placed");
        problem.slots.flags(p.start).week
    };

    assert_eq!(week_of(0), 0, "cohort A's exam sits in A's exam week");
    assert_eq!(week_of(1), 1, "cohort B's in B's");
}
