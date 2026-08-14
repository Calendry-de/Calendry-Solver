//! The immutable problem instance the search runs against.
//!
//! Built once per run from a caller snapshot, then never mutated. All string
//! ids have been resolved to dense indices by this point.

use crate::ids::{GroupIdx, OfferingIdx, PersonIdx, PlacementIdx, RoomIdx, SlotIdx};
use crate::slots::SlotTable;

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
}

#[derive(Clone, Debug)]
pub struct Offering {
    pub id: String,
    pub kind: String,
    pub required_session_count: u32,
    pub duration_blocks: u32,
    pub lecturers: Vec<PersonIdx>,
    pub groups: Vec<GroupIdx>,
    pub participants: Vec<PersonIdx>,
    /// Rooms that satisfy this Offering's features, capacity, online policy and
    /// explicit allow-list. Precomputed at load — the search never re-filters.
    pub eligible_rooms: Vec<RoomIdx>,
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
///
/// `reference` is the caller-supplied "now", resolved against the grid. It is
/// `None` only when the caller's reference lies past the end of the term, in
/// which case every Session in the snapshot is in the past.
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

/// Occupancy the solver must respect but may not move.
#[derive(Clone, Debug)]
pub struct FixedOccupancy {
    pub session_id: String,
    pub room: Option<RoomIdx>,
    pub start: SlotIdx,
    pub duration_blocks: u32,
    pub lecturers: Vec<PersonIdx>,
    pub groups: Vec<GroupIdx>,
    pub reason: Immovable,
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

/// Which constraint types are switched on for this run.
///
/// Only the two the v1 slice implements are represented. Adding a type here is
/// deliberately a code change, not configuration — there is no interpreter.
#[derive(Clone, Debug, Default)]
pub struct ConstraintSet {
    pub room_double_booking: Option<String>,
    pub exact_frequency: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Problem {
    pub slots: SlotTable,
    pub rooms: Vec<Room>,
    pub groups: Vec<Group>,
    pub persons: Vec<Person>,
    pub offerings: Vec<Offering>,
    pub placements: Vec<PlacementVar>,
    pub fixed: Vec<FixedOccupancy>,
    pub constraints: ConstraintSet,
}

impl Problem {
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
}

/// Resolve `parent_id` links into indices, rejecting cycles.
///
/// The ancestor/descendant closure is derived here rather than transmitted:
/// shipping the app's closure table would create a second source of truth the
/// solver has no way to check. Cycle detection matters even in the v1 slice,
/// where the closure itself is unused, because a cycle would hang the walk the
/// moment group double-booking lands.
pub fn resolve_group_parents(
    parent_of: &[Option<GroupIdx>],
) -> Result<(), GroupCycle> {
    let n = parent_of.len();
    // 0 = unvisited, 1 = on current path, 2 = settled.
    let mut state = vec![0u8; n];

    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        let mut path = Vec::new();
        let mut cur = Some(GroupIdx(start as u32));

        while let Some(g) = cur {
            match state[g.get()] {
                1 => {
                    let at = path.iter().position(|&x: &GroupIdx| x == g).unwrap_or(0);
                    return Err(GroupCycle(path[at..].to_vec()));
                }
                2 => break,
                _ => {}
            }
            state[g.get()] = 1;
            path.push(g);
            cur = parent_of[g.get()];
        }

        for g in path {
            state[g.get()] = 2;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct GroupCycle(pub Vec<GroupIdx>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_tree() {
        // 0 <- 1 <- 2, and 3 a second root.
        let parents = vec![None, Some(GroupIdx(0)), Some(GroupIdx(1)), None];
        assert!(resolve_group_parents(&parents).is_ok());
    }

    #[test]
    fn rejects_a_cycle() {
        let parents = vec![Some(GroupIdx(1)), Some(GroupIdx(0))];
        assert!(resolve_group_parents(&parents).is_err());
    }

    #[test]
    fn rejects_a_self_parent() {
        let parents = vec![Some(GroupIdx(0))];
        assert!(resolve_group_parents(&parents).is_err());
    }
}
