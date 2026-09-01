//! `DifferentTime` — the first Offering relation built on ADR-0028's
//! mechanism (issue #50). Two coverage angles, matching every other
//! structural type in this codebase: the search must never CREATE the
//! violation (construction), and the authoritative checker must still
//! REPORT one that already exists in locked input (`evaluate_hard`), per
//! ADR-0014.

use calendry_solver_core::constraints::{ConstraintType, evaluate_hard};
use calendry_solver_core::ids::OfferingIdx;
use calendry_solver_core::problem::{Problem, ProblemSpec, RelationKind, RelationSpec};
use calendry_solver_core::search::construct;
use calendry_solver_core::solution::Solution;
use calendry_solver_core::testing::{fixture, grid, offering, room, structural_room_only};

fn two_offerings_different_rooms() -> Problem {
    // 2 slots, 2 Rooms — Room occupancy alone could never prevent A and B
    // from sharing a slot, since each is eligible for a Room the other is
    // not. Only the relation can force them apart.
    let a = offering("a", 1, &[0]);
    let b = offering("b", 1, &[1]);
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![a, b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::DifferentTime,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    Problem::build(spec).unwrap()
}

#[test]
fn construction_never_lets_related_offerings_share_a_slot() {
    let problem = two_offerings_different_rooms();
    let (solution, _) = construct(&problem);

    let a = solution.get(problem.placement_ids().next().unwrap());
    let b = solution.get(problem.placement_ids().nth(1).unwrap());
    let (Some(a), Some(b)) = (a, b) else {
        panic!("both Sessions are individually placeable — 2 slots, 2 Rooms, no other conflict");
    };
    assert_ne!(a.start, b.start, "DifferentTime must keep the two Offerings out of the same slot");
}

#[test]
fn unrelated_offerings_are_free_to_share_a_slot() {
    // Same shape, no relation configured — the baseline this type must not
    // regress: two Offerings in different Rooms may perfectly well share a
    // slot when nothing relates them.
    let a = offering("a", 1, &[0]);
    let b = offering("b", 1, &[1]);
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        offerings: vec![a, b],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let (solution, _) = construct(&problem);
    let a = solution
        .get(problem.placement_ids().next().unwrap())
        .unwrap();
    let b = solution
        .get(problem.placement_ids().nth(1).unwrap())
        .unwrap();
    assert_eq!(a.start, b.start, "with nothing relating them, both should land in the first slot");
}

#[test]
fn evaluate_hard_reports_a_relation_already_violated_by_locked_input() {
    // The authoritative checker must catch a clash the search never had a
    // chance to avoid — two ALREADY-LOCKED Sessions of related Offerings,
    // sharing a slot in the input itself. Same reason `structural()` exists
    // independently of `Occupancy` for every other pairwise type (ADR-0014).
    use calendry_solver_core::ids::SlotIdx;
    use calendry_solver_core::problem::{FixedSpec, Immovable};

    let a = offering("a", 0, &[]);
    let b = offering("b", 0, &[]);
    let locked_a = FixedSpec {
        session_id: "sess-a".to_string(),
        external: false,
        offering: Some(OfferingIdx(0)),
        kind: "lecture".to_string(),
        room: None,
        additional_rooms: Default::default(),
        start: SlotIdx(0),
        duration_blocks: 1,
        lecturers: vec![],
        groups: vec![],
        persons: vec![],
        reason: Immovable::Locked,
    };
    let locked_b = FixedSpec {
        session_id: "sess-b".to_string(),
        offering: Some(OfferingIdx(1)),
        ..locked_a.clone()
    };

    let mut spec = ProblemSpec {
        rooms: vec![],
        offerings: vec![a, b],
        fixed: vec![locked_a, locked_b],
        relations: vec![RelationSpec {
            id: "rel-1".to_string(),
            kind: RelationKind::DifferentTime,
            members: vec![OfferingIdx(0), OfferingIdx(1)],
        }],
        ..fixture(grid(2, 1), structural_room_only())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let violations = evaluate_hard(&problem, &Solution::empty(&problem));
    assert!(
        violations
            .iter()
            .any(|v| v.constraint_type == ConstraintType::DifferentTimeRelation
                && v.constraint_id == "rel-1"),
        "expected a DifferentTimeRelation violation, got {violations:?}"
    );
}
