//! Hand-written correctness fixtures.
//!
//! Written by hand and checked in, deliberately kept separate from the
//! parametrized benchmark generator in `calendry-solver-gen`. A generator bug
//! that produced a wrong fixture would be a bug that silently validates itself.

use crate::ids::{GroupIdx, OfferingIdx, PersonIdx, RoomIdx, SlotIdx};
use crate::problem::{
    ConstraintInstance, ConstraintSet, FixedSpec, Group, Immovable, OfferingSpec, Person,
    PlacementVar, Problem, Room,
};
use crate::slots::{SlotTable, WeekKind, WeekSpec};

pub fn teaching_weeks(n: usize) -> Vec<WeekSpec> {
    (0..n)
        .map(|_| WeekSpec {
            kind: WeekKind::Teaching,
            holiday_weekdays: vec![],
        })
        .collect()
}

/// `blocks` blocks on Monday of each of `weeks` weeks.
pub fn grid(blocks: u32, weeks: usize) -> SlotTable {
    SlotTable::build(blocks, &[1], &teaching_weeks(weeks)).unwrap()
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

pub fn rooms(n: u32) -> Vec<Room> {
    (0..n).map(|i| room(&format!("R{i}"))).collect()
}

pub fn group(id: &str, parent: Option<u32>) -> Group {
    Group {
        id: id.to_string(),
        parent: parent.map(GroupIdx),
        name: id.to_string(),
        size: 0,
    }
}

pub fn person(id: &str, groups: &[u32]) -> Person {
    Person {
        id: id.to_string(),
        role_tags: vec!["lecturer".to_string()],
        groups: groups.iter().map(|&g| GroupIdx(g)).collect(),
    }
}

pub fn offering(id: &str, count: u32, eligible: &[u32]) -> OfferingSpec {
    OfferingSpec {
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

pub fn with_groups(mut o: OfferingSpec, groups: &[u32]) -> OfferingSpec {
    o.groups = groups.iter().map(|&g| GroupIdx(g)).collect();
    o
}

pub fn with_lecturers(mut o: OfferingSpec, lecturers: &[u32]) -> OfferingSpec {
    o.lecturers = lecturers.iter().map(|&p| PersonIdx(p)).collect();
    o
}

/// Expand each Offering into `required_session_count` placement variables.
pub fn expand(offerings: &[OfferingSpec]) -> Vec<PlacementVar> {
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

fn inst(id: &str) -> Vec<ConstraintInstance> {
    vec![ConstraintInstance { id: id.to_string(), kinds: vec![] }]
}

/// Every implemented constraint type, applying to all kinds.
pub fn all_constraints() -> ConstraintSet {
    ConstraintSet {
        room_double_booking: inst("c-room"),
        lecturer_double_booking: inst("c-lect"),
        group_double_booking: inst("c-group"),
        person_double_booking: inst("c-person"),
        exact_frequency: inst("c-freq"),
        soft: Vec::new(),
    }
}

/// Room + frequency only — the slice 1 pairing.
pub fn structural_room_only() -> ConstraintSet {
    ConstraintSet {
        room_double_booking: inst("c-room"),
        exact_frequency: inst("c-freq"),
        ..Default::default()
    }
}

/// Group-aware but person-blind: the configuration that *cannot* see a clash
/// between two Groups unrelated in the nesting tree.
pub fn group_only() -> ConstraintSet {
    ConstraintSet {
        room_double_booking: inst("c-room"),
        group_double_booking: inst("c-group"),
        exact_frequency: inst("c-freq"),
        ..Default::default()
    }
}

/// Assemble, expanding placements from the offerings' required counts.
pub fn assemble(
    slots: SlotTable,
    rooms: Vec<Room>,
    groups: Vec<Group>,
    persons: Vec<Person>,
    offerings: Vec<OfferingSpec>,
    fixed: Vec<FixedSpec>,
    constraints: ConstraintSet,
) -> Problem {
    let placements = expand(&offerings);
    Problem::build(slots, rooms, groups, persons, offerings, placements, fixed, constraints)
        .expect("fixture group hierarchy must be acyclic")
}

pub fn fixed_session(id: &str, room: Option<u32>, slot: u32) -> FixedSpec {
    FixedSpec {
        session_id: id.to_string(),
        kind: "lecture".to_string(),
        room: room.map(RoomIdx),
        start: SlotIdx(slot),
        duration_blocks: 1,
        lecturers: vec![],
        groups: vec![],
        persons: vec![],
        reason: Immovable::OutOfScope,
    }
}

pub fn fixed_for_groups(id: &str, room: u32, slot: u32, groups: &[u32]) -> FixedSpec {
    let mut f = fixed_session(id, Some(room), slot);
    f.groups = groups.iter().map(|&g| GroupIdx(g)).collect();
    f
}

// ---------------------------------------------------------------------------
// Slice 1 fixtures
// ---------------------------------------------------------------------------

/// 1 Offering needing 1 Session, 2 rooms, a single slot.
pub fn tiny_problem() -> Problem {
    assemble(
        grid(1, 1),
        rooms(2),
        vec![],
        vec![],
        vec![offering("A", 1, &[0, 1])],
        vec![],
        structural_room_only(),
    )
}

/// 3 Offerings x 1 Session, 3 rooms, 3 slots, with 6 of the 9 room-slot cells
/// blocked so exactly one assignment is feasible:
///
/// ```text
///        S0     S1     S2
///  R0   free   X      X
///  R1   X      free   X
///  R2   X      X      free
/// ```
pub fn forced_unique() -> Problem {
    let mut fixed = Vec::new();
    for r in 0..3u32 {
        for s in 0..3u32 {
            if r != s {
                fixed.push(fixed_session(&format!("blk-r{r}s{s}"), Some(r), s));
            }
        }
    }
    assemble(
        grid(3, 1),
        rooms(3),
        vec![],
        vec![],
        vec![
            offering("A", 1, &[0]),
            offering("B", 1, &[1]),
            offering("C", 1, &[2]),
        ],
        fixed,
        structural_room_only(),
    )
}

/// One Offering demanding 4 Sessions into 3 room-slots.
pub fn oversubscribed() -> Problem {
    assemble(
        grid(3, 1),
        rooms(1),
        vec![],
        vec![],
        vec![offering("A", 4, &[0])],
        vec![],
        structural_room_only(),
    )
}

/// One room, 3 slots, one Offering needing 1 Session. The first slot — the one
/// greedy construction would otherwise take — is occupied by an immovable
/// Session for the given `reason`.
pub fn immovable_blocks_first_slot(reason: Immovable) -> Problem {
    let mut f = fixed_session("pinned", Some(0), 0);
    f.reason = reason;
    assemble(
        grid(3, 1),
        rooms(1),
        vec![],
        vec![],
        vec![offering("A", 1, &[0])],
        vec![f],
        structural_room_only(),
    )
}

/// A symmetric instance with many equally-good placements, so a
/// non-deterministic search would visibly disagree with itself between runs.
pub fn symmetric() -> Problem {
    let all: Vec<u32> = (0..6).collect();
    let offerings: Vec<OfferingSpec> = (0..12)
        .map(|i| offering(&format!("O{i}"), 3, &all))
        .collect();
    assemble(
        SlotTable::build(4, &[1, 2, 3, 4, 5], &teaching_weeks(3)).unwrap(),
        rooms(6),
        vec![],
        vec![],
        offerings,
        vec![],
        structural_room_only(),
    )
}

// ---------------------------------------------------------------------------
// Slice 2 fixtures — nested groups, lecturers, people
// ---------------------------------------------------------------------------

/// Cohort A(0) with two sibling classes B(1) and C(2).
///
/// One slot, two rooms: the siblings **must** be able to meet simultaneously.
/// A symmetric-closure implementation would wrongly block this, because B and C
/// share the ancestor A.
pub fn sibling_classes() -> Problem {
    assemble(
        grid(1, 1),
        rooms(2),
        vec![group("A", None), group("B", Some(0)), group("C", Some(0))],
        vec![],
        vec![
            with_groups(offering("sb", 1, &[0, 1]), &[1]),
            with_groups(offering("sc", 1, &[0, 1]), &[2]),
        ],
        vec![],
        all_constraints(),
    )
}

/// Cohort A(0) -> class B(1). One of them is already fixed at slot 0; the other
/// must be placed. Two rooms and two slots, so only the nested-group rule can
/// force them apart.
///
/// `parent_fixed` selects which direction is exercised.
pub fn parent_child_conflict(parent_fixed: bool) -> Problem {
    let groups = vec![group("A", None), group("B", Some(0))];
    let (fixed_group, placed_group, name) = if parent_fixed {
        (0u32, 1u32, "child-after-parent")
    } else {
        (1u32, 0u32, "parent-after-child")
    };
    assemble(
        grid(2, 1),
        rooms(2),
        groups,
        vec![],
        vec![with_groups(offering(name, 1, &[0, 1]), &[placed_group])],
        vec![fixed_for_groups("pinned", 0, 0, &[fixed_group])],
        all_constraints(),
    )
}

/// A 4-level chain 0 <- 1 <- 2 <- 3, with the root fixed at slot 0 and a
/// session for the leaf needing placement. Confirms the closure is transitive
/// rather than one hop deep.
pub fn deep_chain() -> Problem {
    assemble(
        grid(2, 1),
        rooms(2),
        vec![
            group("L0", None),
            group("L1", Some(0)),
            group("L2", Some(1)),
            group("L3", Some(2)),
        ],
        vec![],
        vec![with_groups(offering("leaf", 1, &[0, 1]), &[3])],
        vec![fixed_for_groups("root-session", 0, 0, &[0])],
        all_constraints(),
    )
}

/// One lecturer leading two Offerings. Two rooms and two slots, so only the
/// lecturer rule can force them apart.
pub fn lecturer_clash() -> Problem {
    assemble(
        grid(2, 1),
        rooms(2),
        vec![],
        vec![person("dr-who", &[])],
        vec![
            with_lecturers(offering("L1", 1, &[0, 1]), &[0]),
            with_lecturers(offering("L2", 1, &[0, 1]), &[0]),
        ],
        vec![],
        all_constraints(),
    )
}

/// **The type-4 case.** Groups X(0) and Y(1) are separate roots — neither is an
/// ancestor or descendant of the other — but one Person belongs to both.
///
/// `GroupDoubleBooking` structurally cannot see this clash. Only
/// `PersonDoubleBooking` can.
pub fn cross_tree_person(constraints: ConstraintSet) -> Problem {
    assemble(
        grid(2, 1),
        rooms(2),
        vec![group("X", None), group("Y", None)],
        vec![
            person("dual-enrolled", &[0, 1]),
            person("only-x", &[0]),
            person("only-y", &[1]),
        ],
        vec![
            with_groups(offering("ox", 1, &[0, 1]), &[0]),
            with_groups(offering("oy", 1, &[0, 1]), &[1]),
        ],
        vec![],
        constraints,
    )
}

// ---------------------------------------------------------------------------
// Slice 3 fixtures — soft constraints
// ---------------------------------------------------------------------------

use crate::rng::Rng;
use crate::slots::WeekKind as WK;
use crate::soft::{SoftInstance, SoftParams};

pub fn room_with(id: &str, rank: u32, is_virtual: bool) -> Room {
    let mut r = room(id);
    r.rank = rank;
    r.is_virtual = is_virtual;
    r
}

pub fn soft(id: &str, weight: f64, params: SoftParams) -> SoftInstance {
    SoftInstance { id: id.to_string(), kinds: vec![], weight, params }
}

/// Structural checks plus the given soft instances.
pub fn with_soft(soft: Vec<SoftInstance>) -> ConstraintSet {
    ConstraintSet { soft, ..all_constraints() }
}

/// One Offering needing one Session, over the given grid and rooms.
pub fn single_session(slots: SlotTable, rooms: Vec<Room>, soft: Vec<SoftInstance>) -> Problem {
    let eligible: Vec<u32> = (0..rooms.len() as u32).collect();
    assemble(
        slots,
        rooms,
        vec![],
        vec![],
        vec![offering("S", 1, &eligible)],
        vec![],
        with_soft(soft),
    )
}

/// **Fixture (a).** 3 blocks on one day, one room, one Session.
///
/// With `MinimizeFirstBlock` and `MinimizeLastBlock` both enabled, block 0 and
/// block 2 each cost `weight` and block 1 costs nothing — so the optimum is
/// **unique and hand-computable**: slot 1, soft cost exactly 0.
pub fn uniquely_optimal_middle_block() -> Problem {
    single_session(
        grid(3, 1),
        rooms(1),
        vec![
            soft("first", 4.0, SoftParams::MinimizeFirstBlock),
            soft("last", 4.0, SoftParams::MinimizeLastBlock),
        ],
    )
}

/// Two weeks: week 0 is an exam week, week 1 is teaching. One block per day,
/// one day, one room — so slot 0 is in the exam week and slot 1 is not.
pub fn exam_week_grid() -> SlotTable {
    SlotTable::build(
        1,
        &[1],
        &[
            WeekSpec { kind: WK::Exam, holiday_weekdays: vec![] },
            WeekSpec { kind: WK::Teaching, holiday_weekdays: vec![] },
        ],
    )
    .unwrap()
}

/// Monday and Saturday, one block each: slot 0 is Monday, slot 1 is Saturday.
pub fn two_day_grid() -> SlotTable {
    SlotTable::build(1, &[1, 6], &teaching_weeks(1)).unwrap()
}

/// A small pseudo-random instance for property tests.
///
/// Deliberately **not** the slice 5 benchmark generator: this exists only to
/// give properties (monotonicity, feasibility, delta agreement) more than one
/// shape to hold over, and correctness fixtures remain hand-written above.
pub fn seeded_instance(seed: u64) -> Problem {
    let mut rng = Rng::new(seed);

    let blocks = 2 + rng.below(3) as u32; // 2..4
    let weeks = 1 + rng.below(2); // 1..2
    let slots = SlotTable::build(blocks, &[1, 2, 6], &teaching_weeks(weeks)).unwrap();

    let n_rooms = 2 + rng.below(3);
    let room_list: Vec<Room> = (0..n_rooms)
        .map(|i| room_with(&format!("R{i}"), 1 + (rng.below(9) as u32), rng.below(4) == 0))
        .collect();

    let n_groups = 1 + rng.below(3);
    let group_list: Vec<Group> = (0..n_groups)
        .map(|i| {
            let parent = if i == 0 || rng.below(2) == 0 { None } else { Some((i - 1) as u32) };
            group(&format!("G{i}"), parent)
        })
        .collect();

    let n_people = 2 + rng.below(4);
    let people: Vec<Person> = (0..n_people)
        .map(|i| person(&format!("P{i}"), &[(rng.below(n_groups)) as u32]))
        .collect();

    let eligible: Vec<u32> = (0..n_rooms as u32).collect();
    let n_off = 2 + rng.below(4);
    let offerings: Vec<OfferingSpec> = (0..n_off)
        .map(|i| {
            let mut o = offering(&format!("O{i}"), 1 + rng.below(2) as u32, &eligible);
            o.groups = vec![GroupIdx(rng.below(n_groups) as u32)];
            o.lecturers = vec![PersonIdx(rng.below(n_people) as u32)];
            o
        })
        .collect();

    let soft_set = vec![
        soft("first", 1.0 + rng.below(4) as f64, SoftParams::MinimizeFirstBlock),
        soft("last", 1.0 + rng.below(4) as f64, SoftParams::MinimizeLastBlock),
        soft("sat", 1.0 + rng.below(4) as f64, SoftParams::MinimizeDayUsage { days: vec![6] }),
        soft("rank", 1.0 + rng.below(4) as f64, SoftParams::MinimizeRoomRank { rank_threshold: 5 }),
        soft("online", 1.0 + rng.below(4) as f64, SoftParams::MinimizeOnline),
    ];

    assemble(slots, room_list, group_list, people, offerings, vec![], with_soft(soft_set))
}
