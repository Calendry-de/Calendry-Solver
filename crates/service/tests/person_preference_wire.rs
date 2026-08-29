//! `Person.preferred` and `PersonPreferenceFit`, from the wire to the objective.
//!
//! Schema 0.7.0 added both, and until now the field crossed the wire and was
//! **dropped**: `build_persons` copied `id`, `role_tags`, `groups` and `blackouts`, so
//! the app's assembly was write-only. The constraint was refused outright. These
//! tests pin the whole path instead, because each half can fail while the other
//! looks healthy — a preference that converts but is never priced, or a rule
//! that is priced against preferences that never arrived, both produce a run
//! that succeeds and honours nothing.
//!
//! The counterpart on the app side is `tests/person-preference-wire.test.ts`,
//! which asserts the same contract from the other direction.

use calendry_solver::convert::{build_output, convert};
use calendry_solver::error::ConvertError;
use calendry_solver_core::ids::{PlacementIdx, SlotIdx};
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_core::{Problem, preferences};
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, scope};

const SEED: u64 = 0xC0FFEE;
const ONLY: PlacementIdx = PlacementIdx(0);

fn rule(weight: f64) -> pb::ConstraintConfig {
    pb::ConstraintConfig {
        weight,
        ..enabled(
            "c-pref",
            pb::constraint_config::Params::PersonPreferenceFit(pb::PersonPreferenceFit {
                // Empty = lecturers only, the decided scope.
                roles: vec![],
            }),
        )
    }
}

fn preference(days: Vec<u32>, blocks: Vec<u32>, multiplier: Option<f64>) -> pb::Preference {
    pb::Preference { days, blocks, weight_multiplier: multiplier, preferred_room_features: vec![] }
}

/// One Offering needing one Session, its single lecturer `p1` stating `pref`.
fn input_with(
    pref: Option<pb::Preference>,
    constraints: Vec<pb::ConstraintConfig>,
) -> pb::SolverInput {
    let mut input = base_input();
    input.persons = vec![pb::Person { preferred: pref, ..common::person("p1") }];
    input.offerings = vec![offering("o1", 1)];
    input.constraints.extend(constraints);
    input
}

fn converted(input: &pb::SolverInput) -> Problem {
    convert(input, &scope(&["o1"])).expect("valid input")
}

// ---------------------------------------------------------------------------
// The field survives conversion
// ---------------------------------------------------------------------------

#[test]
fn a_stated_preference_reaches_the_domain_model() {
    // RED before stage 5: `build_persons` dropped `preferred` silently, so this
    // asserted `Some(..)` against a field that was always `None`.
    let problem =
        converted(&input_with(Some(preference(vec![2, 4], vec![0, 1], Some(1.5))), vec![]));

    let pref = problem.persons[0]
        .preferred
        .as_ref()
        .expect("a stated preference must not be dropped in conversion");
    assert_eq!(pref.days, vec![2, 4]);
    assert_eq!(pref.blocks, vec![0, 1]);
    assert_eq!(pref.weight_multiplier, Some(1.5));
}

#[test]
fn an_absent_multiplier_stays_absent_rather_than_becoming_zero() {
    // The reason the field is `optional` on the wire: proto3's zero default is
    // itself a meaningful multiplier — 0.0 would mean "ignore this person
    // entirely" — so "no override" and "count them at zero" must stay distinct.
    // Coercing one to the other is a silent behaviour change, not a rounding.
    let problem = converted(&input_with(Some(preference(vec![2], vec![], None)), vec![]));

    assert_eq!(
        problem.persons[0]
            .preferred
            .as_ref()
            .unwrap()
            .weight_multiplier,
        None
    );

    // And it behaves as 1.0 rather than as 0.0: the rule still prices this
    // person. A coerced zero would make the cost vanish, which no assertion on
    // the field alone would catch.
    let problem = converted(&input_with(Some(preference(vec![2], vec![], None)), vec![rule(4.0)]));
    let monday = SlotIdx(0);
    assert_eq!(
        problem.preferences.cost(ONLY, monday, &[]),
        4.0,
        "Monday is not the stated Tuesday"
    );
}

#[test]
fn no_stated_preference_is_no_preference_not_an_empty_one() {
    // Inverted emptiness against `Unavailability`, where an empty axis means
    // "every value on that axis". Here nothing stated must cost nothing — the
    // opposite reading would charge every placement in the tenant.
    let problem = converted(&input_with(None, vec![rule(9.0)]));

    assert!(problem.persons[0].preferred.is_none());
    for slot in [SlotIdx(0), SlotIdx(5), SlotIdx(11)] {
        assert_eq!(problem.preferences.cost(ONLY, slot, &[]), 0.0, "slot {slot:?}");
    }
}

// ---------------------------------------------------------------------------
// The constraint is accepted, and priced
// ---------------------------------------------------------------------------

#[test]
fn the_rule_converts_into_its_own_instance_list() {
    // Its own list rather than `set.soft`: a preference cost is keyed by
    // placement, and `SoftModel` is a `(profile, slot, room)` table.
    let mut input = input_with(Some(preference(vec![2], vec![], None)), vec![rule(7.0)]);
    input.constraints.last_mut().unwrap().applies_to_kinds = vec![common::KIND.to_string()];

    let problem = converted(&input);
    assert_eq!(problem.constraints.person_preference_fit.len(), 1);
    let inst = &problem.constraints.person_preference_fit[0];
    assert_eq!(inst.id, "c-pref");
    assert_eq!(inst.weight, 7.0);
    assert_eq!(inst.kinds, vec![common::KIND.to_string()]);
    assert!(problem.soft.instances.is_empty(), "it must not land in the slot-keyed soft model");
}

#[test]
fn the_solve_moves_toward_the_preferred_day() {
    // The end-to-end direction test, and the one that would still fail if every
    // assertion above passed: conversion could be perfect and the term could be
    // computed and steer nothing.
    //
    // `base_input`'s grid is Mon-Fri with 6 blocks over 4 weeks and two rooms,
    // one Session to place — so the only thing that can decide where it goes is
    // this rule.
    let input = input_with(
        Some(preference(vec![4], vec![2], Some(2.0))), // Thursday, block 2
        vec![rule(5.0)],
    );
    let problem = converted(&input);
    let outcome =
        solve(&problem, SEED, Budget { max_wall_millis: 0, max_moves: 20_000 }, &NeverHalt);

    let at = outcome
        .solution
        .get(ONLY)
        .expect("one Session, an empty grid");
    let f = problem.slots.flags(at.start);
    assert_eq!((f.iso_weekday, f.block), (4, 2), "should land on the stated day and block");
    assert_eq!(outcome.objective.soft, 0.0, "a fully satisfied preference costs nothing");
}

#[test]
fn the_component_reaches_the_caller_in_the_objective_breakdown() {
    // The app renders this to explain a score, so an enabled rule has to appear
    // even when it happens to be satisfied — otherwise "configured and
    // satisfied" is indistinguishable from "not configured".
    let input = input_with(Some(preference(vec![4], vec![], Some(2.0))), vec![rule(5.0)]);
    let problem = converted(&input);
    let outcome =
        solve(&problem, SEED, Budget { max_wall_millis: 0, max_moves: 20_000 }, &NeverHalt);
    let output = build_output(&problem, &outcome, 0);

    let objective = output
        .objective
        .as_ref()
        .expect("an objective is always reported");
    let component = objective
        .components
        .iter()
        .find(|c| c.constraint_type == "PersonPreferenceFit")
        .expect("an enabled rule must be reported");
    assert_eq!(component.constraint_id, "c-pref");
}

// ---------------------------------------------------------------------------
// What is still refused
// ---------------------------------------------------------------------------

#[test]
fn scoping_the_rule_to_roles_is_refused_rather_than_approximated() {
    // `roles` exists so the counted set stays decidable without another schema
    // bump; it is not decided. Empty means lecturers only, which is what the
    // solver implements.
    //
    // Refused rather than widened, following the precedent for a scoping axis
    // the solver cannot honour: an offering-scoped constraint row is SKIPPED by
    // the app rather than degraded to unscoped, because degrading it would
    // silently WIDEN the rule. Counting a 200-student cohort's preferences
    // alongside the lecturer's is the same widening.
    let mut input = input_with(Some(preference(vec![2], vec![], None)), vec![rule(5.0)]);
    match input.constraints.last_mut().unwrap().params.as_mut() {
        Some(pb::constraint_config::Params::PersonPreferenceFit(p)) => {
            p.roles = vec!["Student".into()];
        }
        _ => unreachable!("the fixture just pushed this constraint"),
    }

    let e = convert(&input, &scope(&["o1"])).expect_err("a role scope must be refused");
    assert!(
        matches!(&e, ConvertError::PreferenceRolesUnsupported { roles, .. } if roles == &["Student".to_string()]),
        "{e}"
    );
    assert!(e.is_unimplemented(), "nothing the caller can send makes this solvable today");
}

#[test]
fn a_negative_weight_is_refused_like_every_other_soft_weight() {
    // A negative weight would invert the type into "penalize honouring a
    // preference", which it never declared. Same fault class as every other soft
    // weight, so the same variant.
    let input = input_with(Some(preference(vec![2], vec![], None)), vec![rule(-1.0)]);

    let e = convert(&input, &scope(&["o1"])).expect_err("a negative weight must be refused");
    assert!(matches!(e, ConvertError::NegativeSoftWeight { .. }), "{e}");
    assert!(!e.is_unimplemented(), "this is bad input, not an unbuilt feature");
}

#[test]
fn an_out_of_range_multiplier_is_clamped_rather_than_refused() {
    // The asymmetry is deliberate. The app validates the range at its write
    // boundary and a database CHECK backs it up, so an out-of-range value here
    // means one of those was bypassed — by a backfill, or by a peer. This
    // service accepts possibly-invalid input by design and the tenant should
    // still get a timetable, so the bound is enforced by clamping.
    //
    // What must NOT happen is the value reaching `hard_penalty` unclamped,
    // which is what makes this a bound rather than a preference.
    let problem =
        converted(&input_with(Some(preference(vec![2], vec![], Some(50.0))), vec![rule(5.0)]));

    let monday = SlotIdx(0);
    assert_eq!(
        problem.preferences.cost(ONLY, monday, &[]),
        5.0 * preferences::MAX_WEIGHT_MULTIPLIER,
        "50.0 must clamp to the maximum"
    );
    assert!(problem.hard_penalty > problem.preferences.max_cost_per_placement());
}
