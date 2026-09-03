//! `LecturerVeto` combined with a genuine lecturer pool across the wire
//! (Calendry #131).
//!
//! This combination used to be refused as `UNIMPLEMENTED`, because the veto
//! was a per-Offering mask precomputed from `Offering::lecturers` — empty for
//! a pool — and an always-empty hard rule is worse than a refusal. The rule
//! now reads each Person's own mask against the CHOSEN lecturers
//! (`Problem::lecturer_veto_blocks`, ADR-0034's shape), so the combination
//! converts, and what is checked here is that the converted `Problem` carries
//! the per-Person answer rather than the per-Offering one. The rule itself is
//! covered in `crates/core/tests/lecturer_veto_pool.rs`.

use calendry_solver::convert::convert;
use calendry_solver_core::ids::{PersonIdx, SlotIdx};
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, enabled, offering, person, scope};

fn away_at_block(id: &str, block: u32) -> pb::Person {
    pb::Person {
        blackouts: vec![pb::Unavailability {
            days: vec![],
            blocks: vec![block],
            weeks: vec![],
            reason: String::new(),
        }],
        ..person(id)
    }
}

#[test]
fn a_veto_combined_with_a_genuine_pool_is_accepted_and_binds_the_chosen_lecturer() {
    // ABWL's shape from the ticket: two named people, `required = 1`, the
    // tenant's veto on. p1 is away at block 0 of every day, p2 at block 1.
    let mut input = base_input();
    input.persons = vec![away_at_block("p1", 0), away_at_block("p2", 1)];
    input.offerings = vec![pb::Offering {
        candidate_lecturer_ids: vec!["p1".into(), "p2".into()],
        required_lecturer_count: 1,
        ..offering("o1", 1)
    }];
    input
        .constraints
        .push(enabled("c-veto", pb::constraint_config::Params::LecturerVeto(pb::LecturerVeto {})));

    let problem = convert(&input, &scope(&["o1"]))
        .expect("a lecturer veto and a genuine lecturer pool must coexist");

    // The per-Offering mask is EMPTY for the pool — correct, and NOT the veto.
    assert!(
        problem.offerings[0].veto_slots.iter().next().is_none(),
        "a pool Offering has no fixed lecturers to precompute a mask from"
    );

    // The veto lives on the Persons, and the answer depends on who is asked.
    let blocks = |person: u32, slot: u32| {
        problem.lecturer_veto_blocks(std::iter::once(PersonIdx(person)), &[SlotIdx(slot)])
    };
    assert!(blocks(0, 0), "p1 is away at block 0");
    assert!(!blocks(0, 1), "p1 is present at block 1");
    assert!(!blocks(1, 0), "p2 is present at block 0");
    assert!(blocks(1, 1), "p2 is away at block 1");
}

#[test]
fn a_veto_with_no_blackouts_anywhere_costs_nothing_to_ask() {
    // Nobody states a blackout, so the per-Person table is never built and
    // the live check answers `false` from one `is_empty` — the same
    // "costs nothing when unused" property the Room pin has.
    let mut input = base_input();
    input.persons = vec![person("p1"), person("p2")];
    input.offerings = vec![pb::Offering {
        candidate_lecturer_ids: vec!["p1".into(), "p2".into()],
        required_lecturer_count: 1,
        ..offering("o1", 1)
    }];
    input
        .constraints
        .push(enabled("c-veto", pb::constraint_config::Params::LecturerVeto(pb::LecturerVeto {})));

    let problem = convert(&input, &scope(&["o1"])).expect("converts");
    assert!(!problem.lecturer_veto_blocks([PersonIdx(0), PersonIdx(1)].into_iter(), &[SlotIdx(0)]));
}
