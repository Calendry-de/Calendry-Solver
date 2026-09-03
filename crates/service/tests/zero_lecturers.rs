//! `required_lecturer_count: 0` — what it means, and what the solver cannot
//! know (Calendry #130).
//!
//! That ticket reports a real defect: an Offering with no staff attached
//! reaches the solver as "this Session requires zero lecturers", the solver
//! places it, and a complete-looking timetable comes back in which nobody is
//! teaching. Its own summary of why it is silent — *"'this Offering
//! deliberately needs no lecturer' and 'nobody has staffed this yet' are the
//! same wire message"* — is exactly right, and it is the reason this file
//! splits the zero case in two.
//!
//! **Zero required WITH candidates listed is refused.** It reads as "nobody
//! teaches this" and does the opposite: the pool branch in `build_offerings`
//! needs `required >= 1`, so a zero count falls through to the fixed-assignment
//! path and EVERY listed candidate is assigned to EVERY Session. That is the
//! outcome the app's `?? min(1, pool)` derivation exists to prevent — "the
//! solver picks one, never all of them forced onto every Session together" —
//! so accepting it is worse than inert, and reinterpreting it as `1` would
//! silently rewrite the caller's number.
//!
//! **Zero required with an EMPTY pool is accepted**, and that is not an
//! oversight. It is a coherent, enforceable statement: a self-directed study
//! block occupies a Room and a cohort and needs no teacher. Every
//! lecturer-keyed rule simply finds nothing to do —
//! `LecturerDoubleBooking` has no lecturer to clash,
//! `LecturerVeto` no blackout to consult, `PersonPreferenceFit` nobody to
//! count.
//!
//! **And the solver could not disambiguate it even if it wanted to**, which is
//! the finding worth carrying back to the ticket. `required_lecturer_count` is
//! a plain proto3 `uint32`, so it has no field presence: "not stated" and
//! "explicitly zero" are the same bytes on the wire. The information that
//! separates them — whether the tenant left the count NULL or typed 0 — exists
//! only in the app, which is why #130's report entry belongs there. If the
//! distinction is ever wanted HERE, the field has to become `optional uint32`
//! for real presence, the same reasoning `Preference.weight_multiplier` and
//! `RoomFeatureRequirement.min_quantity` are already `optional` for.

use calendry_solver::convert::convert;
use calendry_solver::error::ConvertError;
use calendry_solver_proto::v1 as pb;
use tonic::Code;

mod common;
use common::{base_input, offering, person, scope};

/// One Offering with the given candidate pool and required count.
fn input_with(candidates: &[&str], required: u32) -> pb::SolverInput {
    let mut input = base_input();
    input.persons = candidates.iter().map(|id| person(id)).collect();
    if input.persons.is_empty() {
        // `base_input` needs at least one Person for its other fixtures.
        input.persons = vec![person("p1")];
    }
    input.offerings = vec![pb::Offering {
        candidate_lecturer_ids: candidates.iter().map(|c| (*c).to_string()).collect(),
        required_lecturer_count: required,
        ..offering("o1", 1)
    }];
    input
}

// ---------------------------------------------------------------------------
// Refused: zero required, candidates listed
// ---------------------------------------------------------------------------

#[test]
fn zero_required_with_candidates_is_refused() {
    // The whole point: this is not "nobody teaches this". Before the refusal
    // it produced `lecturers == [p1, p2]` — BOTH candidates on every Session.
    let input = input_with(&["p1", "p2"], 0);

    let error = convert(&input, &scope(&["o1"])).expect_err("this input must be refused");

    assert!(
        matches!(&error, ConvertError::ZeroLecturersRequiredWithCandidates { offering, candidates }
            if offering == "o1" && *candidates == 2),
        "unexpected error: {error}",
    );
    // The message has to say what it actually does, not just that it is
    // rejected — "requires 0 lecturers" looks harmless until you know it
    // assigns all of them.
    let text = error.to_string();
    assert!(text.contains("every Session"), "must name the real effect: {text}");
    assert_eq!(
        tonic::Status::from(error).code(),
        Code::InvalidArgument,
        "bad input the caller can fix, not an unbuilt feature",
    );
}

#[test]
fn one_candidate_and_zero_required_is_refused_too() {
    // Not a pool, and still wrong: a single candidate with a zero count is
    // assigned rather than chosen. The refusal is about the count being zero
    // while staff are attached, not about the pool being large.
    let input = input_with(&["p1"], 0);

    assert!(matches!(
        convert(&input, &scope(&["o1"])),
        Err(ConvertError::ZeroLecturersRequiredWithCandidates { .. })
    ));
}

// ---------------------------------------------------------------------------
// Accepted: zero required, no candidates
// ---------------------------------------------------------------------------

#[test]
fn zero_required_with_an_empty_pool_is_a_genuinely_unstaffed_offering() {
    // Deliberately NOT refused. A study period needs a Room and a cohort and
    // no teacher, and that is a real, enforceable statement — every
    // lecturer-keyed rule simply finds nothing to police.
    //
    // This is also the case #130's app-side derivation lands in, and the
    // solver cannot tell it apart from "not yet staffed": see the module doc.
    let input = input_with(&[], 0);

    let problem = convert(&input, &scope(&["o1"])).expect("an unstaffed Offering is legitimate");

    let o = &problem.offerings[0];
    assert!(o.lecturers.is_empty(), "nobody teaches it");
    assert!(o.eligible_lecturer_combinations.is_empty(), "and there is nothing to choose from");
    assert_eq!(o.lecturer_required_count(), 0);
}

// ---------------------------------------------------------------------------
// Controls — the refusal must not touch the two working shapes
// ---------------------------------------------------------------------------

#[test]
fn a_fixed_assignment_is_unaffected() {
    let input = input_with(&["p1"], 1);
    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    assert_eq!(problem.offerings[0].lecturers.len(), 1, "the one candidate is the assignment");
    assert!(
        problem.offerings[0]
            .eligible_lecturer_combinations
            .is_empty()
    );
}

#[test]
fn a_genuine_pool_is_unaffected() {
    let input = input_with(&["p1", "p2", "p3"], 1);
    let problem = convert(&input, &scope(&["o1"])).expect("valid input");

    let o = &problem.offerings[0];
    assert!(o.lecturers.is_empty(), "a pool has no fixed assignment");
    assert_eq!(o.eligible_lecturer_combinations.len(), 3, "one combination per candidate");
    assert_eq!(o.lecturer_required_count(), 1);
}
