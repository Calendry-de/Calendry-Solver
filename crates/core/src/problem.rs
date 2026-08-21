//! The immutable problem instance the search runs against.
//!
//! Built once per run from a caller snapshot, then never mutated. All string ids
//! have been resolved to dense indices by this point, and every derived set
//! (group closures, attendee lists) is precomputed here rather than in the hot
//! loop.

use crate::aggregates::{Aggregates, ShareInstance};
use crate::bitset::BitSet;
use crate::groups::{GroupClosure, GroupCycle};
use crate::ids::{GroupIdx, OfferingIdx, PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use crate::slots::SlotTable;
use crate::soft::{SoftInstance, SoftModel};

#[derive(Clone, Debug)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub capacity: u32,
    /// Higher = more premium / scarce.
    pub rank: u32,
    pub is_virtual: bool,
    pub features: Vec<String>,
    pub federation_owned: bool,
}

impl Room {
    /// Whether this Room can host only ONE Session per slot.
    ///
    /// True for a physical room, where two Sessions in the same place at the
    /// same time is the definition of a double booking. False for a virtual
    /// one: online delivery is modeled AS a Room so that room-assignment logic
    /// stays uniform, not to make concurrency a scarce resource. Two lectures
    /// streaming at the same hour are not a clash, and there is exactly one
    /// virtual room per delivery mode — so treating it as exclusive caps ALL
    /// online teaching at one Session per slot, institution-wide.
    ///
    /// This is the single definition of that policy. `Occupancy` decides
    /// whether to claim a room's slot bit through it, and `constraints::
    /// check_pair` decides whether to report a shared room through it, so the
    /// search and the report cannot disagree about which rooms are exclusive.
    ///
    /// Note what this is NOT: a virtual room with a genuine concurrency limit
    /// (a single meeting licence, say) cannot be expressed today at all —
    /// `capacity` means seats, and it still gates ELIGIBILITY in `convert`.
    /// Expressing a real cap needs its own field, not an overload of this flag.
    #[inline]
    pub fn is_exclusive(&self) -> bool {
        !self.is_virtual
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
    pub parent: Option<GroupIdx>,
    pub name: String,
    pub size: u32,
}

#[derive(Clone, Debug)]
pub struct Person {
    pub id: String,
    pub role_tags: Vec<String>,
    pub groups: Vec<GroupIdx>,
    /// Windows in which this Person is unavailable. Enforced for Sessions they
    /// LEAD, by `LecturerVeto` — never for Sessions they merely attend.
    pub blackouts: Vec<Unavailability>,
}

/// A blackout window.
///
/// An empty list on an axis means "every value on that axis", so `{days:[5]}`
/// is every Friday and `{blocks:[0]}` is every first block. All three empty
/// therefore means always unavailable, which is the literal reading and is
/// preserved rather than silently treated as "never".
#[derive(Clone, Debug, Default)]
pub struct Unavailability {
    pub days: Vec<u32>,
    pub blocks: Vec<u32>,
    pub weeks: Vec<u32>,
}

impl Unavailability {
    #[inline]
    pub fn matches(&self, f: &crate::slots::SlotFlags) -> bool {
        (self.days.is_empty() || self.days.contains(&f.iso_weekday))
            && (self.blocks.is_empty() || self.blocks.contains(&f.block))
            && (self.weeks.is_empty() || self.weeks.contains(&f.week))
    }
}

/// Why a piece of occupancy cannot be moved.
///
/// Recording *why* rather than just *that* is what makes the deferred v2
/// minimize-movement policy a policy change instead of a rewrite: v2 relaxes
/// exactly one of these variants and no others.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Immovable {
    /// Explicit user lock. Absolute — never relaxed, not even in v2.
    Locked,
    /// Starts before the caller's `reference_slot`. Absolute, correctness rule.
    Past,
    /// Outside the requested scope. The ONLY variant v2 may relax.
    OutOfScope,
    /// Another tenant's use of a Federation-shared Room.
    External,
}

/// Decide whether an existing Session may be moved by this run, and if not, why.
///
/// Precedence is deliberate. `Past` is checked first because past exclusion is
/// unconditional and independent of user intent — a past Session is excluded
/// whether or not anyone locked it. `Locked` outranks `OutOfScope` because a
/// lock is absolute in every version, whereas being out of scope is merely
/// expensive to violate and is exactly what the deferred v2 policy relaxes.
pub fn classify_immovable(
    start: SlotIdx,
    reference: Option<SlotIdx>,
    is_locked: bool,
    in_scope: bool,
) -> Option<Immovable> {
    let is_past = match reference {
        Some(r) => start < r,
        None => true,
    };
    if is_past {
        return Some(Immovable::Past);
    }
    if is_locked {
        return Some(Immovable::Locked);
    }
    if !in_scope {
        return Some(Immovable::OutOfScope);
    }
    None
}

// ---------------------------------------------------------------------------
// Constraint configuration
// ---------------------------------------------------------------------------

/// One configured instance of a constraint type.
///
/// A type can be configured more than once with different `kinds`, which is why
/// this is a list rather than a single optional id.
#[derive(Clone, Debug)]
pub struct ConstraintInstance {
    pub id: String,
    /// Tenant-defined Session/Offering kinds this instance covers.
    /// **Empty means all kinds.**
    pub kinds: Vec<String>,
}

impl ConstraintInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// Which constraint types are switched on, and for which kinds.
///
/// Only the types the solver actually implements are represented. Adding a type
/// here is deliberately a code change, not configuration — there is no
/// interpreter and tenant-supplied logic never executes.
#[derive(Clone, Debug, Default)]
pub struct ConstraintSet {
    pub room_double_booking: Vec<ConstraintInstance>,
    pub lecturer_double_booking: Vec<ConstraintInstance>,
    pub group_double_booking: Vec<ConstraintInstance>,
    pub person_double_booking: Vec<ConstraintInstance>,
    pub exact_frequency: Vec<ConstraintInstance>,
    /// HARD, unary: a lecturer is never assigned during their own blackout.
    /// The blackout VALUES live on `Person.blackouts`; this list only switches
    /// enforcement on.
    pub lecturer_veto: Vec<ConstraintInstance>,
    /// HARD, day-granularity filter.
    pub online_onsite_same_day: Vec<ConstraintInstance>,
    /// HARD, aggregate ratio. Carried on the objective rather than as a filter
    /// — see [`crate::aggregates`].
    pub max_online_share: Vec<ShareInstance>,
    /// The six soft types. Separate from the hard lists because only soft
    /// instances carry a weight and typed parameters.
    pub soft: Vec<SoftInstance>,
}

fn any_covers(list: &[ConstraintInstance], kind: &str) -> bool {
    list.iter().any(|c| c.covers(kind))
}

/// Which structural checks the constructive heuristic should avoid violating
/// for a given kind.
///
/// This is an approximation of the authoritative pairwise rule, and knowingly a
/// conservative one: a violation requires *one instance covering both* sessions'
/// kinds, whereas this asks whether *some* instance covers this kind. The two
/// differ only when a type is configured twice with disjoint kind sets, in which
/// case the heuristic merely avoids a placement it did not strictly need to.
/// Being conservative in the heuristic is safe; the evaluator remains exact.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Enforce {
    pub room: bool,
    pub lecturer: bool,
    pub group: bool,
    pub person: bool,
    pub lecturer_veto: bool,
    pub day_mix: bool,
}

impl ConstraintSet {
    pub fn enforce_for_kind(&self, kind: &str) -> Enforce {
        Enforce {
            room: any_covers(&self.room_double_booking, kind),
            lecturer: any_covers(&self.lecturer_double_booking, kind),
            group: any_covers(&self.group_double_booking, kind),
            person: any_covers(&self.person_double_booking, kind),
            lecturer_veto: any_covers(&self.lecturer_veto, kind),
            day_mix: any_covers(&self.online_onsite_same_day, kind),
        }
    }
}

// ---------------------------------------------------------------------------
// Input specs -> derived problem
// ---------------------------------------------------------------------------

/// An Offering as supplied, before closures are derived.
#[derive(Clone, Debug)]
pub struct OfferingSpec {
    pub id: String,
    pub kind: String,
    pub required_session_count: u32,
    pub duration_blocks: u32,
    pub lecturers: Vec<PersonIdx>,
    pub groups: Vec<GroupIdx>,
    pub participants: Vec<PersonIdx>,
    pub eligible_rooms: Vec<RoomIdx>,
}

/// Immovable occupancy as supplied, before closures are derived.
#[derive(Clone, Debug)]
pub struct FixedSpec {
    pub session_id: String,
    /// The Offering this Session realizes, when it realizes one.
    ///
    /// `None` for ad-hoc Sessions (a `staff_meeting` kind need not realize any
    /// Offering) and for external Federation occupancy, which belongs to another
    /// tenant entirely.
    ///
    /// Load-bearing, not decorative: without it a locked Session cannot be
    /// counted toward its Offering's `ExactFrequency`, and a mid-term re-solve
    /// would schedule the full required count *on top of* the Sessions that
    /// already exist.
    pub offering: Option<OfferingIdx>,
    pub kind: String,
    pub room: Option<RoomIdx>,
    pub start: SlotIdx,
    pub duration_blocks: u32,
    pub lecturers: Vec<PersonIdx>,
    pub groups: Vec<GroupIdx>,
    pub persons: Vec<PersonIdx>,
    pub reason: Immovable,
}

#[derive(Clone, Debug)]
pub struct Offering {
    pub id: String,
    pub kind: String,
    pub required_session_count: u32,
    pub duration_blocks: u32,
    pub lecturers: Vec<PersonIdx>,
    /// The Offering's own Groups, unexpanded. Used to **query** occupancy.
    pub own_groups: Vec<GroupIdx>,
    /// `own_groups` expanded through ancestors and descendants. Used to **mark**
    /// occupancy. See [`crate::groups`] for why only one side expands.
    pub conflict_groups: Vec<GroupIdx>,
    /// Directly-assigned individuals, independent of Group membership. Kept
    /// distinct from `attendees` because output must report who was assigned
    /// individually, not everyone who happens to be in the room.
    pub participants: Vec<PersonIdx>,
    /// Everyone in the room: direct participants plus members of the Groups and
    /// their descendants. Attendance propagates downward only.
    pub attendees: Vec<PersonIdx>,
    pub eligible_rooms: Vec<RoomIdx>,
    pub enforce: Enforce,
    /// Index into the soft cost tables for this Offering's `kind`.
    pub soft_profile: usize,
    /// Slots blocked by the blackouts of this Offering's lecturers. Unary, so
    /// it precomputes into a mask exactly like the soft costs do.
    pub veto_slots: BitSet,
    /// `own_groups` expanded DOWNWARD only. Used by the two Group-scoped
    /// aggregate types, matching attendance semantics: a cohort Session is
    /// attended by its classes, but a class Session does not implicate the
    /// cohort.
    pub subtree_groups: Vec<GroupIdx>,
}

#[derive(Clone, Debug)]
pub struct FixedOccupancy {
    pub session_id: String,
    /// See [`FixedSpec::offering`].
    pub offering: Option<OfferingIdx>,
    pub kind: String,
    pub room: Option<RoomIdx>,
    pub start: SlotIdx,
    pub duration_blocks: u32,
    pub lecturers: Vec<PersonIdx>,
    pub own_groups: Vec<GroupIdx>,
    pub conflict_groups: Vec<GroupIdx>,
    pub attendees: Vec<PersonIdx>,
    pub reason: Immovable,
    pub enforce: Enforce,
    pub subtree_groups: Vec<GroupIdx>,
}

/// One Session that needs placing.
#[derive(Clone, Debug)]
pub struct PlacementVar {
    pub offering: OfferingIdx,
    pub occurrence: u32,
    /// Preserved when this occurrence corresponds to an existing in-scope
    /// Session, so a re-solve does not needlessly churn Session ids downstream.
    pub existing_session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Problem {
    pub slots: SlotTable,
    pub rooms: Vec<Room>,
    pub groups: Vec<Group>,
    pub persons: Vec<Person>,
    pub closure: GroupClosure,
    pub offerings: Vec<Offering>,
    pub placements: Vec<PlacementVar>,
    pub fixed: Vec<FixedOccupancy>,
    pub constraints: ConstraintSet,
    pub soft: SoftModel,
    /// Template for the search's aggregate counters: sized and configured, but
    /// empty. The search clones it and fills it from the fixed occupancy.
    pub aggregate_template: Aggregates,
    /// Derived, never tuned: large enough that one unplaced Session outranks
    /// every reachable soft configuration, so the scalar objective orders
    /// lexicographically without a magic constant.
    pub hard_penalty: f64,
}

impl Problem {
    /// The single derivation path, shared by the service's conversion layer and
    /// by the hand-written test fixtures. Keeping one implementation is what
    /// stops the two from drifting on closure semantics.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        slots: SlotTable,
        rooms: Vec<Room>,
        groups: Vec<Group>,
        persons: Vec<Person>,
        offerings: Vec<OfferingSpec>,
        placements: Vec<PlacementVar>,
        fixed: Vec<FixedSpec>,
        constraints: ConstraintSet,
    ) -> Result<Self, GroupCycle> {
        let parent_of: Vec<Option<GroupIdx>> = groups.iter().map(|g| g.parent).collect();
        let closure = GroupClosure::build(&parent_of)?;

        // Group -> direct members.
        let mut members: Vec<Vec<PersonIdx>> = vec![Vec::new(); groups.len()];
        for (i, p) in persons.iter().enumerate() {
            for g in &p.groups {
                members[g.get()].push(PersonIdx(i as u32));
            }
        }

        let attendees_of = |group_ids: &[GroupIdx], direct: &[PersonIdx]| -> Vec<PersonIdx> {
            let mut out: Vec<PersonIdx> = direct.to_vec();
            for g in closure.expand_subtree(group_ids) {
                out.extend_from_slice(&members[g.get()]);
            }
            out.sort_unstable();
            out.dedup();
            out
        };

        // Distinct kinds in play, so the soft model can build one cost table
        // per profile rather than one per kind.
        let mut kinds: Vec<String> = offerings
            .iter()
            .map(|o| o.kind.clone())
            .chain(fixed.iter().map(|f| f.kind.clone()))
            .collect();
        kinds.sort();
        kinds.dedup();

        let soft = SoftModel::build(constraints.soft.clone(), &slots, &rooms, &kinds);

        // Blackout -> slot mask, resolved against the tenant's grid. Unary, so
        // it precomputes once per Offering.
        let veto_mask = |lecturers: &[PersonIdx]| -> BitSet {
            let mut mask = BitSet::new(slots.len());
            for l in lecturers {
                let blackouts = &persons[l.get()].blackouts;
                if blackouts.is_empty() {
                    continue;
                }
                for slot in slots.all() {
                    let f = slots.flags(slot);
                    if blackouts.iter().any(|b| b.matches(f)) {
                        mask.insert(slot.get());
                    }
                }
            }
            mask
        };

        let derived_offerings = offerings
            .into_iter()
            .map(|o| Offering {
                soft_profile: soft.profile_for_kind(&o.kind),
                veto_slots: veto_mask(&o.lecturers),
                subtree_groups: closure.expand_subtree(&o.groups),
                enforce: constraints.enforce_for_kind(&o.kind),
                conflict_groups: closure.expand_conflict(&o.groups),
                attendees: attendees_of(&o.groups, &o.participants),
                participants: o.participants,
                own_groups: o.groups,
                id: o.id,
                kind: o.kind,
                required_session_count: o.required_session_count,
                duration_blocks: o.duration_blocks,
                lecturers: o.lecturers,
                eligible_rooms: o.eligible_rooms,
            })
            .collect();

        let derived_fixed = fixed
            .into_iter()
            .map(|f| FixedOccupancy {
                enforce: constraints.enforce_for_kind(&f.kind),
                subtree_groups: closure.expand_subtree(&f.groups),
                conflict_groups: closure.expand_conflict(&f.groups),
                attendees: attendees_of(&f.groups, &f.persons),
                own_groups: f.groups,
                session_id: f.session_id,
                offering: f.offering,
                kind: f.kind,
                room: f.room,
                start: f.start,
                duration_blocks: f.duration_blocks,
                lecturers: f.lecturers,
                reason: f.reason,
            })
            .collect();

        // sum(weights) * placements + 1 dominates any achievable soft total, so
        // the scalar objective orders lexicographically. Aggregate violations
        // join `unplaced` on the hard side and are covered by the same bound.
        let hard_penalty = soft.total_weight * placements.len() as f64 + 1.0;

        let aggregate_template = Aggregates::new(
            groups.len(),
            slots.day_count(),
            slots.week_count() as usize,
            constraints.max_online_share.clone(),
        );

        Ok(Self {
            slots,
            rooms,
            groups,
            persons,
            closure,
            offerings: derived_offerings,
            placements,
            fixed: derived_fixed,
            constraints,
            soft,
            aggregate_template,
            hard_penalty,
        })
    }

    #[inline]
    pub fn placement_ids(&self) -> impl Iterator<Item = PlacementIdx> {
        (0..self.placements.len() as u32).map(PlacementIdx)
    }

    #[inline]
    pub fn placement(&self, p: PlacementIdx) -> &PlacementVar {
        &self.placements[p.get()]
    }

    #[inline]
    pub fn offering_of(&self, p: PlacementIdx) -> &Offering {
        &self.offerings[self.placements[p.get()].offering.get()]
    }

    /// A stable label for a placement, preferring the existing Session id.
    pub fn placement_label(&self, p: PlacementIdx) -> String {
        let var = self.placement(p);
        var.existing_session_id
            .clone()
            .unwrap_or_else(|| format!("{}#{}", self.offering_of(p).id, var.occurrence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slots::{SlotTable, WeekKind, WeekSpec};

    fn grid() -> SlotTable {
        SlotTable::build(
            2,
            &[1],
            &[WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] }],
        )
        .unwrap()
    }

    fn group(id: &str, parent: Option<u32>) -> Group {
        Group {
            id: id.to_string(),
            parent: parent.map(GroupIdx),
            name: id.to_string(),
            size: 0,
        }
    }

    #[test]
    fn attendance_pulls_in_descendants_but_not_ancestors() {
        // A(0) -> B(1) -> C(2)
        let groups = vec![group("A", None), group("B", Some(0)), group("C", Some(1))];
        let persons = vec![
            Person { id: "pa".into(), role_tags: vec![], groups: vec![GroupIdx(0)], blackouts: vec![] },
            Person { id: "pb".into(), role_tags: vec![], groups: vec![GroupIdx(1)], blackouts: vec![] },
            Person { id: "pc".into(), role_tags: vec![], groups: vec![GroupIdx(2)], blackouts: vec![] },
        ];

        let specs = vec![
            OfferingSpec {
                id: "top".into(), kind: "lecture".into(), required_session_count: 1,
                duration_blocks: 1, lecturers: vec![], groups: vec![GroupIdx(0)],
                participants: vec![], eligible_rooms: vec![],
            },
            OfferingSpec {
                id: "leaf".into(), kind: "lecture".into(), required_session_count: 1,
                duration_blocks: 1, lecturers: vec![], groups: vec![GroupIdx(2)],
                participants: vec![], eligible_rooms: vec![],
            },
        ];

        let p = Problem::build(
            grid(), vec![], groups, persons, specs, vec![], vec![],
            ConstraintSet::default(),
        )
        .unwrap();

        // A session for the cohort involves everyone beneath it.
        assert_eq!(
            p.offerings[0].attendees,
            vec![PersonIdx(0), PersonIdx(1), PersonIdx(2)]
        );
        // A session for the deepest group involves only its own member.
        assert_eq!(p.offerings[1].attendees, vec![PersonIdx(2)]);

        // But conflict propagation still goes BOTH ways.
        assert!(p.offerings[1].conflict_groups.contains(&GroupIdx(0)));
    }

    #[test]
    fn kind_scoping_selects_which_checks_apply() {
        let set = ConstraintSet {
            group_double_booking: vec![ConstraintInstance {
                id: "g".into(),
                kinds: vec!["lecture".into()],
            }],
            room_double_booking: vec![ConstraintInstance { id: "r".into(), kinds: vec![] }],
            ..Default::default()
        };

        // A groupless tenant kind is not subject to the group check...
        let staff = set.enforce_for_kind("staff_meeting");
        assert!(!staff.group);
        // ...but an all-kinds instance still applies.
        assert!(staff.room);

        let lecture = set.enforce_for_kind("lecture");
        assert!(lecture.group && lecture.room);
    }
}
