//! Hand-written correctness fixtures.
//!
//! These are written by hand and checked in, deliberately kept separate from
//! the parametrized benchmark generator in `calendry-solver-gen`. A generator
//! bug that produced a wrong fixture would be a bug that silently validates
//! itself.

use crate::ids::{OfferingIdx, RoomIdx, SlotIdx};
use crate::problem::{ConstraintSet, FixedOccupancy, Immovable, Offering, PlacementVar, Problem, Room};
use crate::slots::{SlotTable, WeekKind, WeekSpec};

pub fn teaching_weeks(n: usize) -> Vec<WeekSpec> {
    (0..n)
        .map(|_| WeekSpec {
            kind: WeekKind::Teaching,
            holiday_weekdays: vec![],
        })
        .collect()
}

pub fn room(id: &str) -> Room {
    Room {
        id: id.to_string(),
        name: id.to_string(),
        capacity: 30,
        rank: 1,
        is_virtual: false,
        features: vec![],
        federation_owned: false,
    }
}

pub fn offering(id: &str, count: u32, eligible: &[u32]) -> Offering {
    Offering {
        id: id.to_string(),
        kind: "lecture".to_string(),
        required_session_count: count,
        duration_blocks: 1,
        lecturers: vec![],
        groups: vec![],
        participants: vec![],
        eligible_rooms: eligible.iter().map(|&r| RoomIdx(r)).collect(),
    }
}

/// Expand each Offering into `required_session_count` placement variables.
pub fn expand(offerings: &[Offering]) -> Vec<PlacementVar> {
    let mut out = Vec::new();
    for (i, o) in offerings.iter().enumerate() {
        for occ in 0..o.required_session_count {
            out.push(PlacementVar {
                offering: OfferingIdx(i as u32),
                occurrence: occ,
                existing_session_id: None,
            });
        }
    }
    out
}

pub fn both_constraints() -> ConstraintSet {
    ConstraintSet {
        room_double_booking: Some("c-room".to_string()),
        exact_frequency: Some("c-freq".to_string()),
    }
}

fn assemble(
    slots: SlotTable,
    rooms: Vec<Room>,
    offerings: Vec<Offering>,
    fixed: Vec<FixedOccupancy>,
) -> Problem {
    let placements = expand(&offerings);
    Problem {
        slots,
        rooms,
        groups: vec![],
        persons: vec![],
        offerings,
        placements,
        fixed,
        constraints: both_constraints(),
    }
}

/// 1 Offering needing 1 Session, 2 rooms, a single slot.
pub fn tiny_problem() -> Problem {
    let slots = SlotTable::build(1, &[1], &teaching_weeks(1)).unwrap();
    assemble(
        slots,
        vec![room("R0"), room("R1")],
        vec![offering("A", 1, &[0, 1])],
        vec![],
    )
}

fn blocker(id: &str, room: u32, slot: u32) -> FixedOccupancy {
    FixedOccupancy {
        session_id: id.to_string(),
        room: Some(RoomIdx(room)),
        start: SlotIdx(slot),
        duration_blocks: 1,
        lecturers: vec![],
        groups: vec![],
        reason: Immovable::OutOfScope,
    }
}

/// **Test 1 fixture.** 3 Offerings x 1 Session, 3 rooms, 3 slots, with 6 of the
/// 9 room-slot cells blocked so that exactly one assignment is feasible:
///
/// ```text
///        S0     S1     S2
///  R0   free   X      X
///  R1   X      free   X
///  R2   X      X      free
/// ```
///
/// Each Offering is eligible for exactly one room, so the packing is forced:
/// A->(R0,S0), B->(R1,S1), C->(R2,S2).
pub fn forced_unique() -> Problem {
    // 1 week, Monday only, 3 blocks/day => 3 slots.
    let slots = SlotTable::build(3, &[1], &teaching_weeks(1)).unwrap();

    let mut fixed = Vec::new();
    for r in 0..3u32 {
        for s in 0..3u32 {
            if r != s {
                fixed.push(blocker(&format!("blk-r{r}s{s}"), r, s));
            }
        }
    }

    assemble(
        slots,
        vec![room("R0"), room("R1"), room("R2")],
        vec![
            offering("A", 1, &[0]),
            offering("B", 1, &[1]),
            offering("C", 1, &[2]),
        ],
        fixed,
    )
}

/// **Test 2 fixture.** One Offering demanding 4 Sessions into 3 room-slots.
pub fn oversubscribed() -> Problem {
    let slots = SlotTable::build(3, &[1], &teaching_weeks(1)).unwrap();
    assemble(slots, vec![room("R0")], vec![offering("A", 4, &[0])], vec![])
}

/// **Tests 3 & 4 fixture.** One room, 3 slots, one Offering needing 1 Session.
/// The first slot — the one greedy construction would otherwise take — is
/// occupied by an immovable Session for the given `reason`.
pub fn immovable_blocks_first_slot(reason: Immovable) -> Problem {
    let slots = SlotTable::build(3, &[1], &teaching_weeks(1)).unwrap();
    let fixed = vec![FixedOccupancy {
        session_id: "pinned".to_string(),
        room: Some(RoomIdx(0)),
        start: SlotIdx(0),
        duration_blocks: 1,
        lecturers: vec![],
        groups: vec![],
        reason,
    }];
    assemble(slots, vec![room("R0")], vec![offering("A", 1, &[0])], fixed)
}

/// A symmetric instance with many equally-good placements, so that a
/// non-deterministic search would visibly disagree with itself between runs.
pub fn symmetric() -> Problem {
    let slots = SlotTable::build(4, &[1, 2, 3, 4, 5], &teaching_weeks(3)).unwrap();
    let rooms: Vec<Room> = (0..6).map(|i| room(&format!("R{i}"))).collect();
    let all: Vec<u32> = (0..6).collect();
    let offerings: Vec<Offering> = (0..12)
        .map(|i| offering(&format!("O{i}"), 3, &all))
        .collect();
    assemble(slots, rooms, offerings, vec![])
}
