//! Hand-written correctness fixtures.
//!
//! Written by hand and checked in, deliberately kept separate from the
//! parametrized benchmark generator in `calendry-solver-gen`. A generator bug
//! that produced a wrong fixture would be a bug that silently validates itself.
//!
//! # Organization
//!
//! Grouped by the **behaviour** each fixture exists to exercise, not by the
//! implementation slice that introduced it. The sections used to read "Slice 1
//! fixtures", "Slice 4 fixtures", which is a record of *when* code was written:
//! a maintainer adding a share-cap fixture had no principled home for it, so the
//! file could only grow by appending.
//!
//! * **Builders** — grid, room, group, person, offering, spec assembly.
//! * **Constraint sets** — the named configurations fixtures select from.
//! * **Structural** — room, lecturer, group and person double-booking, and the
//!   nested-group closure.
//! * **Unary** — the six soft types and `LecturerVeto`: slot-keyed lookups with
//!   O(1) exact deltas.
//! * **Preference** — `PersonPreferenceFit`, which is per-placement rather than
//!   slot-keyed because it depends on who leads the placement.
//! * **Aggregate** — `OnlineOnsiteSameDay` and `MaxOnlineShare`, which are not
//!   expressible as a slot-keyed bitset.
//! * **Seeded** — randomized instances for the drift and determinism tests.

use crate::aggregates::DayMixInstance;
use crate::ids::{GroupIdx, OfferingIdx, PersonIdx, RoomIdx, SlotIdx};
use crate::preferences::{Preference, PreferenceInstance};
use crate::problem::{
    ConstraintInstance, ConstraintSet, FixedSpec, Group, Immovable, OfferingSpec, Person,
    PlacementVar, Problem, ProblemSpec, Room, SchedulingPattern,
};
use crate::slots::{SlotTable, WeekKind, WeekSpec};
use crate::solution::MAX_ADDITIONAL_ROOMS;

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

pub fn teaching_weeks(n: usize) -> Vec<WeekSpec> {
    (0..n)
        .map(|_| WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] })
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
        location: String::new(),
    }
}

/// `room` with a real `location` — most fixtures leave it empty, which is
/// inert for `MinimizeLocationChange`; a test exercising it needs Rooms in
/// genuinely different locations.
pub fn room_at(id: &str, location: &str) -> Room {
    Room { location: location.to_string(), ..room(id) }
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
        blackouts: vec![],
    }
}

/// `group` with a real headcount — most fixtures leave `size: 0`, which is
/// inert for `GroupSizeFitsRoom`; a test exercising it needs a nonzero value.
pub fn group_with_size(id: &str, parent: Option<u32>, size: u32) -> Group {
    Group { size, ..group(id, parent) }
}

pub fn person(id: &str, groups: &[u32]) -> Person {
    Person {
        id: id.to_string(),
        role_tags: vec!["lecturer".to_string()],
        groups: groups.iter().map(|&g| GroupIdx(g)).collect(),
        blackouts: vec![],
        preferred: None,
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
        required_room_count: 0,
        eligible_room_combinations: vec![],
        min_capacity: 0,
        scheduling_pattern: SchedulingPattern::Unspecified,
    }
}

/// `offering` with its scheduling pattern overridden — the fixtures
/// exercising `DistributedPatternAdherence`/`BlockPatternAdherence` need this;
/// every other fixture stays at the default `Unspecified`.
pub fn with_pattern(mut o: OfferingSpec, pattern: SchedulingPattern) -> OfferingSpec {
    o.scheduling_pattern = pattern;
    o
}

/// `offering` turned multi-Room: `required_room_count` Rooms per Session,
/// with every combination of that many distinct Rooms out of `pool`
/// eligible — the simplest possible combination set for a fixture, mirroring
/// what `convert::build_offerings` would compute for a Room pool with no
/// capacity or feature filtering in play.
pub fn with_room_combinations(
    mut o: OfferingSpec,
    required_room_count: u32,
    pool: &[u32],
) -> OfferingSpec {
    o.required_room_count = required_room_count;
    o.eligible_room_combinations = combinations(pool, required_room_count as usize)
        .into_iter()
        .map(|combo| {
            let mut additional = [None; MAX_ADDITIONAL_ROOMS];
            for (slot, &r) in additional.iter_mut().zip(&combo[1..]) {
                *slot = Some(RoomIdx(r));
            }
            (RoomIdx(combo[0]), additional)
        })
        .collect();
    o
}

/// Every combination of `k` distinct elements of `pool`, in ascending order.
fn combinations(pool: &[u32], k: usize) -> Vec<Vec<u32>> {
    if k == 0 || k > pool.len() {
        return vec![];
    }
    if k == pool.len() {
        return vec![pool.to_vec()];
    }
    let mut out = combinations(&pool[1..], k - 1);
    for combo in &mut out {
        combo.insert(0, pool[0]);
    }
    out.extend(combinations(&pool[1..], k));
    out
}

pub fn with_groups(mut o: OfferingSpec, groups: &[u32]) -> OfferingSpec {
    o.groups = groups.iter().map(|&g| GroupIdx(g)).collect();
    o
}

pub fn with_lecturers(mut o: OfferingSpec, lecturers: &[u32]) -> OfferingSpec {
    o.lecturers = lecturers.iter().map(|&p| PersonIdx(p)).collect();
    o
}

pub fn with_min_capacity(mut o: OfferingSpec, min_capacity: u32) -> OfferingSpec {
    o.min_capacity = min_capacity;
    o
}

/// Expand each Offering into `required_session_count` placement variables.
///
/// Thin wrapper over [`ProblemSpec::expand_placements`], kept because a few
/// tests want the variables without a whole spec.
pub fn expand(offerings: &[OfferingSpec]) -> Vec<PlacementVar> {
    let mut out = Vec::new();
    for (i, o) in offerings.iter().enumerate() {
        for occ in 0..o.required_session_count {
            out.push(PlacementVar {
                offering: OfferingIdx(i as u32),
                occurrence: occ,
                existing_session_id: None,
                original: None,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Constraint sets
// ---------------------------------------------------------------------------

fn inst(id: &str) -> Vec<ConstraintInstance> {
    vec![ConstraintInstance { id: id.to_string(), kinds: vec![] }]
}

fn day_mix(id: &str, weight: f64) -> Vec<DayMixInstance> {
    vec![DayMixInstance { id: id.to_string(), kinds: vec![], weight }]
}

/// Every implemented constraint type except `PersonPreferenceFit`, applying to
/// all kinds.
///
/// The exception is deliberate. No fixture built on this has a Person with a
/// stated preference, so switching the rule on here would put an enabled rule
/// with nothing to work with into most of the suite — the `lecturer_veto`
/// shape, which "looked healthy and could never fire". Fixtures that mean to
/// exercise it say so, via [`with_preference`].
pub fn all_constraints() -> ConstraintSet {
    ConstraintSet {
        room_double_booking: inst("c-room"),
        lecturer_double_booking: inst("c-lect"),
        group_double_booking: inst("c-group"),
        person_double_booking: inst("c-person"),
        exact_frequency: inst("c-freq"),
        lecturer_veto: inst("c-veto"),
        // Included, unlike `person_preference_fit` below, and the asymmetry is
        // the precedent `lecturer_veto` already set: a veto with no declared
        // windows produces an EMPTY mask, so it cannot change any fixture's
        // outcome — where an enabled soft rule with nothing to say still lands
        // an inert term in the objective of most of the suite. Keeping it on
        // here means every existing test also asserts that switching group
        // vetoes on changes nothing when no Group has declared anything.
        group_veto: inst("c-group-veto"),
        // Same asymmetry, same precedent: every fixture's Group defaults to
        // `size: 0`, so this can never fire unless a test sets a real size.
        group_size_fits_room: inst("c-group-size"),
        max_concurrent_online_sessions: Vec::new(),
        minimize_capacity_waste: Vec::new(),
        protected_block: Vec::new(),
        max_consecutive_blocks: Vec::new(),
        max_daily_span: Vec::new(),
        max_weekly_teaching_load: Vec::new(),
        exam_spacing_same_day: Vec::new(),
        exam_spacing_window: Vec::new(),
        minimize_weekday_imbalance: Vec::new(),
        minimize_location_change: Vec::new(),
        // Weight 5 mirrors the app catalogue's `defaultWeight` for this type,
        // so a fixture's day-mix cost reads the same as a real tenant's.
        online_onsite_same_day: day_mix("c-mix", 5.0),
        max_online_share: Vec::new(),
        person_preference_fit: Vec::new(),
        soft: Vec::new(),
        compactness: Vec::new(),
        distributed_pattern_adherence: Vec::new(),
        block_pattern_adherence: Vec::new(),
    }
}

/// Room double-booking + frequency only: the minimal structural pairing.
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

/// Assemble a fixture, expanding placements from the offerings' required counts.
///
/// Takes a [`ProblemSpec`], so a fixture names only the fields it cares about
/// instead of padding out seven positional arguments — five of the six call
/// sites in this crate's integration tests used to pass two or three `vec![]`
/// fillers, and one passed three.
///
/// Every Offering is in scope: these fixtures build the whole instance from
/// nothing, so there is no out-of-scope region for a lock policy to protect.
/// Scope is exercised at the conversion boundary, where a real request supplies
/// one.
pub fn assemble(mut spec: ProblemSpec) -> Problem {
    spec.expand_placements();
    Problem::build(spec).expect("fixture group hierarchy must be acyclic")
}

/// A spec on `slots` with the given constraint set and nothing else.
///
/// The common shape of a fixture: `fixture(grid(1, 1), structural_room_only())`
/// then override the two or three fields that matter.
pub fn fixture(slots: SlotTable, constraints: ConstraintSet) -> ProblemSpec {
    ProblemSpec { constraints, ..ProblemSpec::new(slots) }
}

pub fn fixed_session(id: &str, room: Option<u32>, slot: u32) -> FixedSpec {
    FixedSpec {
        session_id: id.to_string(),
        // These fixtures are pure occupancy blockers, not realizations of an
        // Offering under test. Frequency accounting is exercised at the
        // conversion boundary, where the link is actually resolved.
        offering: None,
        kind: "lecture".to_string(),
        room: room.map(RoomIdx),
        additional_rooms: [None; MAX_ADDITIONAL_ROOMS],
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
// Structural — room double-booking, immovability, symmetry
// ---------------------------------------------------------------------------

/// 1 Offering needing 1 Session, 2 rooms, a single slot.
pub fn tiny_problem() -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(2),
        offerings: vec![offering("A", 1, &[0, 1])],
        constraints: structural_room_only(),
        ..ProblemSpec::new(grid(1, 1))
    })
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
    assemble(ProblemSpec {
        rooms: rooms(3),
        offerings: vec![
            offering("A", 1, &[0]),
            offering("B", 1, &[1]),
            offering("C", 1, &[2]),
        ],
        fixed,
        constraints: structural_room_only(),
        ..ProblemSpec::new(grid(3, 1))
    })
}

/// One Offering demanding 4 Sessions into 3 room-slots.
pub fn oversubscribed() -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(1),
        offerings: vec![offering("A", 4, &[0])],
        constraints: structural_room_only(),
        ..ProblemSpec::new(grid(3, 1))
    })
}

/// One room, 3 slots, one Offering needing 1 Session. The first slot — the one
/// greedy construction would otherwise take — is occupied by an immovable
/// Session for the given `reason`.
pub fn immovable_blocks_first_slot(reason: Immovable) -> Problem {
    let mut f = fixed_session("pinned", Some(0), 0);
    f.reason = reason;
    assemble(ProblemSpec {
        rooms: rooms(1),
        offerings: vec![offering("A", 1, &[0])],
        fixed: vec![f],
        constraints: structural_room_only(),
        ..ProblemSpec::new(grid(3, 1))
    })
}

/// A symmetric instance with many equally-good placements, so a
/// non-deterministic search would visibly disagree with itself between runs.
pub fn symmetric() -> Problem {
    let all: Vec<u32> = (0..6).collect();
    let offerings: Vec<OfferingSpec> = (0..12)
        .map(|i| offering(&format!("O{i}"), 3, &all))
        .collect();
    assemble(ProblemSpec {
        rooms: rooms(6),
        offerings,
        constraints: structural_room_only(),
        ..ProblemSpec::new(SlotTable::build(4, &[1, 2, 3, 4, 5], &teaching_weeks(3)).unwrap())
    })
}

// ---------------------------------------------------------------------------
// Structural — nested groups, lecturers, cross-tree people
// ---------------------------------------------------------------------------

/// Cohort A(0) with two sibling classes B(1) and C(2).
///
/// One slot, two rooms: the siblings **must** be able to meet simultaneously.
/// A symmetric-closure implementation would wrongly block this, because B and C
/// share the ancestor A.
pub fn sibling_classes() -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(2),
        groups: vec![group("A", None), group("B", Some(0)), group("C", Some(0))],
        offerings: vec![
            with_groups(offering("sb", 1, &[0, 1]), &[1]),
            with_groups(offering("sc", 1, &[0, 1]), &[2]),
        ],
        constraints: all_constraints(),
        ..ProblemSpec::new(grid(1, 1))
    })
}

/// Two Sessions, one slot, and exactly ONE room — so the room is the only thing
/// that could keep them apart.
///
/// `virtual_room` selects which kind of room it is, and that is the whole point:
/// a virtual room hosts unlimited concurrent Sessions, a physical one hosts one.
/// The two groups are unrelated roots, so no group rule interferes.
pub fn two_sessions_one_room(virtual_room: bool) -> Problem {
    assemble(ProblemSpec {
        rooms: vec![room_with("R", 1, virtual_room)],
        groups: vec![group("A", None), group("B", None)],
        offerings: vec![
            with_groups(offering("a", 1, &[0]), &[0]),
            with_groups(offering("b", 1, &[0]), &[1]),
        ],
        constraints: all_constraints(),
        ..ProblemSpec::new(grid(1, 1))
    })
}

/// The same collision, but already present in IMMOVABLE input.
///
/// The search can never *create* a room clash, so the reporting path is only
/// reachable through Sessions the caller pinned there — which "warn and allow"
/// permits. Nothing is placeable here; the instance exists to be evaluated.
pub fn two_fixed_sessions_one_room(virtual_room: bool) -> Problem {
    assemble(ProblemSpec {
        rooms: vec![room_with("R", 1, virtual_room)],
        groups: vec![group("A", None)],
        fixed: vec![
            fixed_session("pinned-a", Some(0), 0),
            fixed_session("pinned-b", Some(0), 0),
        ],
        constraints: all_constraints(),
        ..ProblemSpec::new(grid(1, 1))
    })
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
    assemble(ProblemSpec {
        rooms: rooms(2),
        groups,
        offerings: vec![with_groups(offering(name, 1, &[0, 1]), &[placed_group])],
        fixed: vec![fixed_for_groups("pinned", 0, 0, &[fixed_group])],
        constraints: all_constraints(),
        ..ProblemSpec::new(grid(2, 1))
    })
}

/// A 4-level chain 0 <- 1 <- 2 <- 3, with the root fixed at slot 0 and a
/// session for the leaf needing placement. Confirms the closure is transitive
/// rather than one hop deep.
pub fn deep_chain() -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(2),
        groups: vec![
            group("L0", None),
            group("L1", Some(0)),
            group("L2", Some(1)),
            group("L3", Some(2)),
        ],
        offerings: vec![with_groups(offering("leaf", 1, &[0, 1]), &[3])],
        fixed: vec![fixed_for_groups("root-session", 0, 0, &[0])],
        constraints: all_constraints(),
        ..ProblemSpec::new(grid(2, 1))
    })
}

/// One lecturer leading two Offerings. Two rooms and two slots, so only the
/// lecturer rule can force them apart.
pub fn lecturer_clash() -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(2),
        persons: vec![person("dr-who", &[])],
        offerings: vec![
            with_lecturers(offering("L1", 1, &[0, 1]), &[0]),
            with_lecturers(offering("L2", 1, &[0, 1]), &[0]),
        ],
        constraints: all_constraints(),
        ..ProblemSpec::new(grid(2, 1))
    })
}

/// **The type-4 case.** Groups X(0) and Y(1) are separate roots — neither is an
/// ancestor or descendant of the other — but one Person belongs to both.
///
/// `GroupDoubleBooking` structurally cannot see this clash. Only
/// `PersonDoubleBooking` can.
pub fn cross_tree_person(constraints: ConstraintSet) -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(2),
        groups: vec![group("X", None), group("Y", None)],
        persons: vec![
            person("dual-enrolled", &[0, 1]),
            person("only-x", &[0]),
            person("only-y", &[1]),
        ],
        offerings: vec![
            with_groups(offering("ox", 1, &[0, 1]), &[0]),
            with_groups(offering("oy", 1, &[0, 1]), &[1]),
        ],
        constraints,
        ..ProblemSpec::new(grid(2, 1))
    })
}

// ---------------------------------------------------------------------------
// Unary — the six soft types
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
    assemble(ProblemSpec {
        rooms,
        offerings: vec![offering("S", 1, &eligible)],
        constraints: with_soft(soft),
        ..ProblemSpec::new(slots)
    })
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

/// The reverse of [`exam_week_grid`]: slot 0 is teaching, slot 1 is the exam
/// week. Greedy construction's "earliest slot" default therefore lands
/// OUTSIDE the exam week here, which is what lets a test show
/// `MinimizeExamWeek { invert: true }` actually MOVE a Session into the exam
/// week rather than merely fail to move it out — `exam_week_grid` alone
/// cannot demonstrate that, since its unweighted default already sits on the
/// exam week by coincidence of slot order.
pub fn teaching_then_exam_grid() -> SlotTable {
    SlotTable::build(
        1,
        &[1],
        &[
            WeekSpec { kind: WK::Teaching, holiday_weekdays: vec![] },
            WeekSpec { kind: WK::Exam, holiday_weekdays: vec![] },
        ],
    )
    .unwrap()
}

/// Monday and Saturday, one block each: slot 0 is Monday, slot 1 is Saturday.
pub fn two_day_grid() -> SlotTable {
    SlotTable::build(1, &[1, 6], &teaching_weeks(1)).unwrap()
}

/// One week, entirely `Break`. The institution is closed for the whole term.
pub fn break_week_grid() -> SlotTable {
    SlotTable::build(1, &[1], &[WeekSpec { kind: WeekKind::Break, holiday_weekdays: vec![] }])
        .unwrap()
}

/// Slot 0 is a `Break` week, slot 1 is `Teaching` — the reverse of
/// [`teaching_then_exam_grid`], for the same reason: it lets a test show the
/// search actively AVOIDING the closed slot in favour of the open one, rather
/// than merely never having a reason to leave a default that already avoided
/// it by luck of slot order.
pub fn break_then_teaching_grid() -> SlotTable {
    SlotTable::build(
        1,
        &[1],
        &[
            WeekSpec { kind: WeekKind::Break, holiday_weekdays: vec![] },
            WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] },
        ],
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// Preference — PersonPreferenceFit, keyed by placement rather than by slot
// ---------------------------------------------------------------------------

/// A stated preference. Empty arrays mean **no preference on that axis**, the
/// inverse of [`blackout`].
pub fn preference(days: &[u32], blocks: &[u32], multiplier: Option<f64>) -> Preference {
    Preference {
        days: days.to_vec(),
        blocks: blocks.to_vec(),
        room_features: Vec::new(),
        weight_multiplier: multiplier,
    }
}

/// A stated room-type preference, with no day/block axis — the shape a
/// lecturer who cares only about the room, not the time, would state.
pub fn room_preference(features: &[&str], multiplier: Option<f64>) -> Preference {
    Preference {
        days: Vec::new(),
        blocks: Vec::new(),
        room_features: features.iter().map(ToString::to_string).collect(),
        weight_multiplier: multiplier,
    }
}

pub fn person_with_preference(id: &str, groups: &[u32], pref: Preference) -> Person {
    Person { preferred: Some(pref), ..person(id, groups) }
}

pub fn preference_rule(id: &str, weight: f64) -> PreferenceInstance {
    PreferenceInstance { id: id.to_string(), kinds: vec![], weight }
}

/// Structural checks plus the given `PersonPreferenceFit` instances.
pub fn with_preference(rules: Vec<PreferenceInstance>) -> ConstraintSet {
    ConstraintSet { person_preference_fit: rules, ..all_constraints() }
}

/// The weight [`two_lecturers_with_opposing_preferences`] is configured with.
pub const PREFERENCE_WEIGHT: f64 = 8.0;

/// **The discriminating fixture for the combination rule.** One Session, two
/// required lecturers whose multipliers *and* whose satisfaction both differ —
/// on Monday lecturer 0 gets what they asked for and lecturer 1 does not, and on
/// Saturday the reverse.
///
/// Built this way because a fixture with equal multipliers or equal fits makes
/// the three candidate combination rules agree, and so proves nothing. Here
/// they disagree, and disagree in *shape* rather than only in magnitude:
///
/// | form | Monday | Saturday |
/// |---|---|---|
/// | mean of the product — CORRECT | `1.0 * w` | `0.25 * w` |
/// | sum instead of mean | `2.0 * w` | `0.5 * w` |
/// | `mean(m) * mean(unmet)` | `0.625 * w` | `0.625 * w` |
///
/// The separated form prices both days identically, so it does not merely give
/// a different number — it loses the preference between the two slots
/// altogether.
pub fn two_lecturers_with_opposing_preferences() -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(1),
        // Monday is slot 0, Saturday is slot 1.
        persons: vec![
            person_with_preference("half", &[], preference(&[1], &[], Some(0.5))),
            person_with_preference("double", &[], preference(&[6], &[], Some(2.0))),
        ],
        offerings: vec![with_lecturers(offering("S", 1, &[0]), &[0, 1])],
        constraints: with_preference(vec![preference_rule("c-pref", PREFERENCE_WEIGHT)]),
        ..ProblemSpec::new(two_day_grid())
    })
}

/// One Session whose Offering requires **no** lecturer, with the rule enabled.
///
/// `required_lecturer_count` is a `uint32` defaulting to 0, so this is reachable
/// rather than theoretical — a tenant-defined `staff_meeting` kind is the real
/// case. The counted set is empty, the mean is undefined, and the cost must be
/// 0 from an all-zero table row rather than from a branch at read time.
pub fn no_lecturers_with_preference_enabled() -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(1),
        persons: vec![person_with_preference(
            "nobody",
            &[],
            preference(&[1], &[], Some(2.0)),
        )],
        offerings: vec![offering("S", 1, &[0])],
        constraints: with_preference(vec![preference_rule("c-pref", PREFERENCE_WEIGHT)]),
        ..ProblemSpec::new(two_day_grid())
    })
}

/// Two rooms, one lecturer stating ONLY a room-type preference — no day or
/// block axis at all. Room 0 has the wanted feature, Room 1 does not.
///
/// The day/block-only shape: this is the case `narrow()` returns `None` for,
/// which is exactly why room preference cannot ride on `counted` — a
/// lecturer who said nothing about days or blocks is not absent from the
/// room axis, they simply never stated one to begin with.
pub fn one_lecturer_wanting_a_room_feature(multiplier: Option<f64>) -> Problem {
    assemble(ProblemSpec {
        rooms: vec![
            Room { features: vec!["lab".to_string()], ..room("R0") },
            room("R1"),
        ],
        persons: vec![person_with_preference(
            "wants-lab",
            &[],
            room_preference(&["lab"], multiplier),
        )],
        offerings: vec![with_lecturers(offering("S", 1, &[0, 1]), &[0])],
        constraints: with_preference(vec![preference_rule("c-pref", PREFERENCE_WEIGHT)]),
        ..ProblemSpec::new(two_day_grid())
    })
}

// ---------------------------------------------------------------------------
// Seeded — randomized instances for the drift and determinism tests
// ---------------------------------------------------------------------------

/// A small pseudo-random instance for property tests.
///
/// Deliberately **not** the benchmark generator in `calendry-solver-gen`: this
/// exists only to give properties (monotonicity, feasibility, delta agreement)
/// more than one shape to hold over. Correctness fixtures stay hand-written.
pub fn seeded_instance(seed: u64) -> Problem {
    seeded(seed, false)
}

/// The same shapes, with `PersonPreferenceFit` enabled and roughly half the
/// people stating a preference — for the drift assertion, which otherwise would
/// never see the term at all.
///
/// Some Offerings get a second lecturer here, so the multi-lecturer mean is
/// exercised under the drift assertion and not only by the hand-computed
/// fixture.
pub fn seeded_preference_instance(seed: u64) -> Problem {
    seeded(seed, true)
}

/// The preference draws come from their own RNG, consumed after the main one is
/// finished with, so `seeded_instance(seed)` produces byte-identical instances
/// to before this fixture existed.
fn seeded(seed: u64, preferences: bool) -> Problem {
    let mut rng = Rng::new(seed);

    let blocks = 2 + rng.below(3) as u32; // 2..4
    let weeks = 1 + rng.below(2); // 1..2
    let slots = SlotTable::build(blocks, &[1, 2, 6], &teaching_weeks(weeks)).unwrap();

    // Fixed, small vocabulary rather than a random string: a room-type
    // preference matches by KEY, so the drift/room tests need real chances of
    // both a match and a miss, which a vocabulary of one made of unique
    // strings could never produce.
    const ROOM_FEATURE_VOCAB: [&str; 3] = ["lab", "av", "whiteboard"];

    let n_rooms = 2 + rng.below(3);
    let room_list: Vec<Room> = (0..n_rooms)
        .map(|i| {
            let mut r = room_with(&format!("R{i}"), 1 + (rng.below(9) as u32), rng.below(4) == 0);
            r.features = ROOM_FEATURE_VOCAB
                .iter()
                .filter(|_| rng.below(2) == 0)
                .map(ToString::to_string)
                .collect();
            r
        })
        .collect();

    let n_groups = 1 + rng.below(3);
    let group_list: Vec<Group> = (0..n_groups)
        .map(|i| {
            let parent = if i == 0 || rng.below(2) == 0 { None } else { Some((i - 1) as u32) };
            group(&format!("G{i}"), parent)
        })
        .collect();

    let n_people = 2 + rng.below(4);
    let mut people: Vec<Person> = (0..n_people)
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
        soft(
            "rank",
            1.0 + rng.below(4) as f64,
            SoftParams::MinimizeRoomRank { rank_threshold: 5, invert: false },
        ),
        soft("online", 1.0 + rng.below(4) as f64, SoftParams::MinimizeOnline),
    ];

    let mut constraints = with_soft(soft_set);

    let mut offerings = offerings;
    if preferences {
        let mut prng = Rng::new(seed ^ 0x9e37_79b9_7f4a_7c15);

        for (i, p) in people.iter_mut().enumerate() {
            let wants_day_block = prng.below(2) == 1;
            // Independent of the day/block draw, deliberately: a person who
            // stated ONLY a room preference (no day/block at all) is the case
            // `narrow()` returns `None` for, which is exactly the gap
            // `room_wanted` exists to not silently drop. If this were gated by
            // `wants_day_block` too, that lecturer would never appear in this
            // fixture and the drift test would cover a term it cannot see.
            let wants_room = prng.below(3) == 0;
            if !wants_day_block && !wants_room {
                continue;
            }

            // One day the grid teaches on, so the value is not narrowed away;
            // the block axis is stated only sometimes, which is what makes the
            // divisor 1 for some people and 2 for others.
            let days = if wants_day_block { vec![[1u32, 2, 6][i % 3]] } else { vec![] };
            let blocks = if !wants_day_block || prng.below(2) == 0 {
                vec![]
            } else {
                vec![prng.below(2) as u32]
            };
            let room_features: Vec<String> = if wants_room {
                ROOM_FEATURE_VOCAB
                    .iter()
                    .filter(|_| prng.below(2) == 0)
                    .map(ToString::to_string)
                    .collect()
            } else {
                vec![]
            };
            let multiplier = match prng.below(3) {
                0 => None,
                1 => Some(0.5),
                _ => Some(2.0),
            };
            p.preferred =
                Some(Preference { days, blocks, room_features, weight_multiplier: multiplier });
        }

        for o in &mut offerings {
            if prng.below(2) == 0 {
                let extra = PersonIdx(prng.below(n_people) as u32);
                if !o.lecturers.contains(&extra) {
                    o.lecturers.push(extra);
                }
            }
        }

        constraints.person_preference_fit =
            vec![preference_rule("c-pref", 1.0 + prng.below(4) as f64)];
    }

    assemble(ProblemSpec {
        rooms: room_list,
        groups: group_list,
        persons: people,
        offerings,
        constraints,
        ..ProblemSpec::new(slots)
    })
}

// ---------------------------------------------------------------------------
// Unary — lecturer blackouts; Aggregate — day mix and share ratio
// ---------------------------------------------------------------------------

use crate::aggregates::{ShareInstance, ShareWindow};
use crate::problem::Unavailability;

pub fn blackout(days: &[u32], blocks: &[u32], weeks: &[u32]) -> Unavailability {
    Unavailability { days: days.to_vec(), blocks: blocks.to_vec(), weeks: weeks.to_vec() }
}

/// A Group unavailable in the given calendar WEEKS, every day and block.
///
/// The shape an academic calendar actually produces: "this cohort runs the first
/// six weeks of the Term" arrives as the COMPLEMENT of that range, with both
/// other axes empty meaning "all values on that axis". Days and blocks are
/// equally expressible — it is the same `Unavailability` a Person carries — but
/// no caller has wanted a Group away only on Fridays, so this does not invent an
/// interface for it.
pub fn group_away_in_weeks(id: &str, parent: Option<u32>, weeks: &[u32]) -> Group {
    group_with_blackouts(id, parent, vec![blackout(&[], &[], weeks)])
}

/// The general form, mirroring [`person_with_blackouts`].
pub fn group_with_blackouts(id: &str, parent: Option<u32>, b: Vec<Unavailability>) -> Group {
    Group { blackouts: b, ..group(id, parent) }
}

/// Structural checks with group vetoes switched OFF, for the inertness test.
pub fn without_group_veto() -> ConstraintSet {
    ConstraintSet { group_veto: Vec::new(), ..all_constraints() }
}

/// **The discriminating pair for blackout DIRECTION**, built as one hierarchy
/// so both halves read from the same picture.
///
/// `cohort` (index 0) is the parent; `seminar` (index 1) is its child. The
/// cohort is away on Monday. Two Offerings, each attached to one of them, on a
/// grid whose only two slots are Monday and Saturday:
///
/// | Offering attached to | must avoid Monday? | why |
/// |---|---|---|
/// | `seminar` — the CHILD of the absent group | YES | a blackout binds descendants |
/// | `cohort` — the group that is itself absent | YES | its own window |
///
/// and the mirror case, [`seminar_away_cohort_free`], where the CHILD is the one
/// away and the parent's Offering must stay free to sit on Monday.
///
/// A flat fixture cannot tell `expand_ancestry` from `expand_subtree` or
/// `expand_conflict`: with no hierarchy all three return the same set. This pair
/// is the only reason the direction is pinned rather than assumed.
pub fn cohort_away_seminar_bound() -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(1),
        groups: vec![
            group_with_blackouts("cohort", None, vec![blackout(&[1], &[], &[])]),
            group("seminar", Some(0)),
        ],
        offerings: vec![with_groups(offering("S", 1, &[0]), &[1])],
        constraints: all_constraints(),
        ..ProblemSpec::new(two_day_grid())
    })
}

/// The mirror of [`cohort_away_seminar_bound`]: the CHILD is away, and the
/// parent's Offering must remain placeable on Monday.
///
/// This is the half that fails against a downward expansion, and the failure it
/// prevents is concrete: one seminar on block placement would veto the lecture
/// its whole cohort attends.
pub fn seminar_away_cohort_free() -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(1),
        groups: vec![
            group("cohort", None),
            group_with_blackouts("seminar", Some(0), vec![blackout(&[1], &[], &[])]),
        ],
        offerings: vec![with_groups(offering("S", 1, &[0]), &[0])],
        constraints: all_constraints(),
        ..ProblemSpec::new(two_day_grid())
    })
}

pub fn person_with_blackouts(id: &str, groups: &[u32], b: Vec<Unavailability>) -> Person {
    let mut p = person(id, groups);
    p.blackouts = b;
    p
}

pub fn share_rule(id: &str, max_ratio: f64, window: ShareWindow) -> ShareInstance {
    ShareInstance { id: id.to_string(), kinds: vec![], max_ratio, window }
}

/// Structural + the given `MaxOnlineShare` rules.
pub fn with_share(rules: Vec<ShareInstance>) -> ConstraintSet {
    ConstraintSet { max_online_share: rules, ..all_constraints() }
}

/// Everything except the day-mix rule, so a test can show what happens without it.
pub fn without_day_mix() -> ConstraintSet {
    ConstraintSet { online_onsite_same_day: vec![], ..all_constraints() }
}

/// Everything except lecturer vetoes.
pub fn without_lecturer_veto() -> ConstraintSet {
    ConstraintSet { lecturer_veto: vec![], ..all_constraints() }
}

/// One virtual room and one on-site room, in that order, so greedy reaches for
/// the online one first.
pub fn online_first_rooms() -> Vec<Room> {
    vec![
        room_with("R-online", 1, true),
        room_with("R-onsite", 1, false),
    ]
}

/// A lecturer blacked out on the first block. Two blocks, one room, one Session.
pub fn lecturer_blacked_out_on_first_block(constraints: ConstraintSet) -> Problem {
    assemble(ProblemSpec {
        rooms: rooms(1),
        persons: vec![person_with_blackouts(
            "dr-busy",
            &[],
            vec![blackout(&[], &[0], &[])],
        )],
        offerings: vec![with_lecturers(offering("S", 1, &[0]), &[0])],
        constraints,
        ..ProblemSpec::new(grid(2, 1))
    })
}

/// One Group, two Sessions on a single day, one of which cannot go online.
///
/// `GroupDoubleBooking` forces the two into different blocks, and greedy reaches
/// for the virtual room first (`online_first_rooms` lists it first) — so without
/// the day-mix rule the free Session goes online and the on-site-only one does
/// not: a mixed day. With the rule, both end up on-site, which is reachable
/// because the on-site room is free all day.
///
/// The mix comes from **eligibility**, not from occupancy. An earlier version
/// pinned a Session into the virtual room to make it unavailable at one block,
/// which only worked because virtual rooms were wrongly treated as capacity-1;
/// once that bug was fixed the virtual room was free at both blocks and greedy
/// put *both* Sessions online. A fixture must not depend on the defect its
/// neighbours are testing around.
pub fn group_day_with_both_room_types(constraints: ConstraintSet) -> Problem {
    assemble(ProblemSpec {
        rooms: online_first_rooms(),
        groups: vec![group("G", None)],
        offerings: vec![
            // Free to go either way; greedy takes the virtual room.
            with_groups(offering("either", 1, &[0, 1]), &[0]),
            // Not permitted online at all — the on-site room only.
            with_groups(offering("onsite-only", 1, &[1]), &[0]),
        ],
        constraints,
        // One day, two blocks.
        ..ProblemSpec::new(grid(2, 1))
    })
}

/// A hand-built Solution for [`group_day_with_both_room_types`] that DOES mix:
/// the flexible Session online, the on-site-only one beside it, same day.
///
/// Constructed rather than searched for, because the point of the test using it
/// is what a mixed day COSTS — and a test that first has to coax the search into
/// producing one would be measuring the search instead of the price.
pub fn solution_mixing_one_day(problem: &Problem) -> crate::solution::Solution {
    use crate::solution::{Placement, Solution};

    let mut solution = Solution::empty(problem);

    for p in problem.placement_ids() {
        let offering = problem.offering_of(p);
        // Room 0 is virtual in `online_first_rooms`, room 1 is physical.
        let room = if offering.id == "either" { RoomIdx(0) } else { RoomIdx(1) };
        let start = if offering.id == "either" { SlotIdx(0) } else { SlotIdx(1) };

        solution.set(p, Some(Placement::single(start, room)));
    }

    solution
}

/// One Group with four Sessions across four blocks of one day, with an online
/// room available. `max_ratio` caps how many may be online.
pub fn share_capped_group(rules: Vec<ShareInstance>) -> Problem {
    assemble(ProblemSpec {
        rooms: online_first_rooms(),
        groups: vec![group("G", None)],
        offerings: vec![with_groups(offering("S", 4, &[0, 1]), &[0])],
        // Day-mix would forbid mixing modes on the single day, which would mask
        // what the share cap is doing, so it is deliberately off here.
        constraints: ConstraintSet { max_online_share: rules, ..without_day_mix() },
        ..ProblemSpec::new(SlotTable::build(4, &[1], &teaching_weeks(1)).unwrap())
    })
}

/// Two weeks, two Sessions per week, one Group. Under `PER_TERM` a 50% cap allows
/// two online anywhere; under `PER_WEEK` it allows at most one per week.
pub fn share_across_two_weeks(rules: Vec<ShareInstance>) -> Problem {
    assemble(ProblemSpec {
        rooms: online_first_rooms(),
        groups: vec![group("G", None)],
        offerings: vec![with_groups(offering("S", 4, &[0, 1]), &[0])],
        constraints: ConstraintSet { max_online_share: rules, ..without_day_mix() },
        ..ProblemSpec::new(SlotTable::build(2, &[1], &teaching_weeks(2)).unwrap())
    })
}

/// A seeded instance that exercises the aggregate counters: nested groups,
/// virtual rooms, several weeks, and both aggregate rules enabled.
pub fn seeded_aggregate_instance(seed: u64) -> Problem {
    let mut rng = Rng::new(seed);

    let blocks = 2 + rng.below(2) as u32;
    let weeks = 2 + rng.below(2);
    let slots = SlotTable::build(blocks, &[1, 2], &teaching_weeks(weeks)).unwrap();

    let room_list = vec![
        room_with("V0", 1, true),
        room_with("R1", 3, false),
        room_with("R2", 7, false),
    ];

    let groups = vec![
        group("G0", None),
        group("G1", Some(0)),
        group("G2", Some(0)),
    ];

    let n_people = 2 + rng.below(3);
    let people: Vec<Person> = (0..n_people)
        .map(|i| {
            let mut p = person(&format!("P{i}"), &[(1 + rng.below(2)) as u32]);
            if rng.below(3) == 0 {
                p.blackouts = vec![blackout(&[], &[rng.below(blocks as usize) as u32], &[])];
            }
            p
        })
        .collect();

    let offerings: Vec<OfferingSpec> = (0..3 + rng.below(3))
        .map(|i| {
            let mut o = offering(&format!("O{i}"), 1 + rng.below(3) as u32, &[0, 1, 2]);
            o.groups = vec![GroupIdx(rng.below(3) as u32)];
            o.lecturers = vec![PersonIdx(rng.below(n_people) as u32)];
            o
        })
        .collect();

    let constraints = ConstraintSet {
        max_online_share: vec![share_rule(
            "share",
            0.25 * (1 + rng.below(3)) as f64,
            if rng.below(2) == 0 { ShareWindow::PerTerm } else { ShareWindow::PerWeek },
        )],
        soft: vec![
            soft("first", 2.0, SoftParams::MinimizeFirstBlock),
            soft("online", 3.0, SoftParams::MinimizeOnline),
        ],
        ..all_constraints()
    };

    assemble(ProblemSpec {
        rooms: room_list,
        groups,
        persons: people,
        offerings,
        constraints,
        ..ProblemSpec::new(slots)
    })
}
