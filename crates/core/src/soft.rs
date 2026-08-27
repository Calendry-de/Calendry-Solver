//! Soft constraints and the weighted objective.
//!
//! # All six soft types are unary
//!
//! Every soft type depends only on the `(slot, room)` of a **single** Session —
//! unlike all four structural types, which are pairwise. That has two
//! consequences the design leans on heavily:
//!
//! 1. Soft cost is a **precomputable table** indexed by `(profile, slot, room)`,
//!    so scoring a candidate placement is one indexed read.
//! 2. The soft delta of a move is **exact and O(1)**, not an approximation,
//!    which is what makes millions of evaluations viable on CPU.
//!
//! A *profile* is the set of soft instances applying to a given tenant `kind`.
//! Tenants typically have one or two distinct profiles, so the tables are small.

use std::collections::HashMap;

use crate::ids::{RoomIdx, SlotIdx};
use crate::problem::Room;
use crate::slots::{SlotFlags, SlotTable, WeekKind};

/// Typed parameters, one variant per predefined soft type. There is no
/// interpreter: tenant-supplied logic never executes.
#[derive(Clone, Debug, PartialEq)]
pub enum SoftParams {
    MinimizeFirstBlock,
    MinimizeLastBlock,
    /// Penalize the listed ISO weekdays (1 = Monday). Generalizes the
    /// prototype's hardcoded "minimize Saturday": with tenant-configured
    /// `active_days`, Saturday is not structurally special.
    MinimizeDayUsage {
        days: Vec<u32>,
    },
    /// `Room.rank` is ordered **higher = more premium/scarce**; rooms at or
    /// above the threshold are penalized.
    MinimizeRoomRank {
        rank_threshold: u32,
    },
    MinimizeExamWeek,
    MinimizeOnline,
}

impl SoftParams {
    pub fn type_name(&self) -> &'static str {
        match self {
            SoftParams::MinimizeFirstBlock => "MinimizeFirstBlock",
            SoftParams::MinimizeLastBlock => "MinimizeLastBlock",
            SoftParams::MinimizeDayUsage { .. } => "MinimizeDayUsage",
            SoftParams::MinimizeRoomRank { .. } => "MinimizeRoomRank",
            SoftParams::MinimizeExamWeek => "MinimizeExamWeek",
            SoftParams::MinimizeOnline => "MinimizeOnline",
        }
    }

    /// The single predicate used by **both** the cost table and the final
    /// breakdown, so the fast path and the reported counts cannot disagree.
    #[inline]
    pub fn applies(&self, f: &SlotFlags, room: &Room) -> bool {
        match self {
            SoftParams::MinimizeFirstBlock => f.is_first_block,
            SoftParams::MinimizeLastBlock => f.is_last_block,
            SoftParams::MinimizeDayUsage { days } => days.contains(&f.iso_weekday),
            SoftParams::MinimizeRoomRank { rank_threshold } => room.rank >= *rank_threshold,
            SoftParams::MinimizeExamWeek => f.week_kind == WeekKind::Exam,
            SoftParams::MinimizeOnline => room.is_virtual,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SoftInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    /// Non-negative. Zero means "report the count but do not steer".
    pub weight: f64,
    pub params: SoftParams,
}

impl SoftInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// Precomputed soft-cost lookup, plus the profile assignment per kind.
#[derive(Clone, Debug, Default)]
pub struct SoftModel {
    pub instances: Vec<SoftInstance>,
    /// profile -> indices into `instances`. Retained for diagnostics and to
    /// make the profile assignment inspectable in tests.
    profiles: Vec<Vec<usize>>,
    /// profile -> flat `slot * n_rooms + room` costs.
    tables: Vec<Vec<f64>>,
    profile_of_kind: HashMap<String, usize>,
    n_rooms: usize,
    /// Sum of every instance weight — the basis for the derived hard penalty.
    pub total_weight: f64,
}

impl SoftModel {
    pub fn build(
        instances: Vec<SoftInstance>,
        slots: &SlotTable,
        rooms: &[Room],
        kinds: &[String],
    ) -> Self {
        let total_weight = instances.iter().map(|i| i.weight).sum();
        let n_rooms = rooms.len();

        // One profile per distinct set of applicable instances. Kinds sharing a
        // profile share a table.
        let mut profiles: Vec<Vec<usize>> = Vec::new();
        let mut profile_of_kind: HashMap<String, usize> = HashMap::new();

        let resolve = |kind: &str, profiles: &mut Vec<Vec<usize>>| {
            let applicable: Vec<usize> = instances
                .iter()
                .enumerate()
                .filter(|(_, i)| i.covers(kind))
                .map(|(n, _)| n)
                .collect();
            match profiles.iter().position(|p| *p == applicable) {
                Some(p) => p,
                None => {
                    profiles.push(applicable);
                    profiles.len() - 1
                }
            }
        };

        for kind in kinds {
            let p = resolve(kind, &mut profiles);
            profile_of_kind.insert(kind.clone(), p);
        }
        // Profile 0 must always exist so unknown kinds have somewhere to land.
        if profiles.is_empty() {
            profiles.push(Vec::new());
        }

        let tables = profiles
            .iter()
            .map(|members| {
                let mut t = vec![0.0f64; slots.len() * n_rooms.max(1)];
                if n_rooms == 0 {
                    return t;
                }
                for slot in slots.all() {
                    let f = slots.flags(slot);
                    for (r, room) in rooms.iter().enumerate() {
                        // Fixed iteration order keeps the sum bit-reproducible;
                        // f64 addition is not associative.
                        let mut c = 0.0;
                        for &m in members {
                            if instances[m].params.applies(f, room) {
                                c += instances[m].weight;
                            }
                        }
                        t[slot.get() * n_rooms + r] = c;
                    }
                }
                t
            })
            .collect();

        Self { instances, profiles, tables, profile_of_kind, n_rooms, total_weight }
    }

    pub fn profile_for_kind(&self, kind: &str) -> usize {
        self.profile_of_kind.get(kind).copied().unwrap_or(0)
    }

    #[inline]
    pub fn cost(&self, profile: usize, slot: SlotIdx, room: RoomIdx) -> f64 {
        if self.n_rooms == 0 || self.tables.is_empty() {
            return 0.0;
        }
        self.tables[profile][slot.get() * self.n_rooms + room.get()]
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Which instances apply to a profile. Distinct profiles mean distinct
    /// tables, so this is how a test confirms kind scoping took effect.
    pub fn profile_members(&self, profile: usize) -> &[usize] {
        &self.profiles[profile]
    }

    pub fn profile_count(&self) -> usize {
        self.profiles.len()
    }
}

/// The objective, kept as two components rather than one opaque number.
///
/// `unplaced` is the only hard dimension the search can trade: repair never
/// places into an occupied slot, so it cannot create a structural clash. A
/// Session it fails to place stays unplaced and surfaces as an `ExactFrequency`
/// violation.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct Objective {
    pub unplaced: u32,
    /// Violated `MaxOnlineShare` cells. Joins `unplaced` on the hard side
    /// because it is an aggregate ratio that cannot be enforced as a filter.
    pub aggregate: u32,
    pub soft: f64,
}

impl Objective {
    /// Lexicographic ordering expressed as a scalar, so SA has a Δ to work with.
    ///
    /// `hard_penalty` is **derived** from the instance — see
    /// [`crate::problem::Problem::hard_penalty`] — rather than being a tuned
    /// magic constant, and is large enough that one hard violation outranks
    /// every reachable soft configuration.
    #[inline]
    pub fn hard(&self) -> u32 {
        self.unplaced + self.aggregate
    }

    #[inline]
    pub fn total(&self, hard_penalty: f64) -> f64 {
        self.hard() as f64 * hard_penalty + self.soft
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SoftComponent {
    pub constraint_id: String,
    pub constraint_type: &'static str,
    pub raw_count: u64,
    pub weighted: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slots::WeekSpec;

    fn room_at(rank: u32, virt: bool) -> Room {
        Room {
            id: "r".into(),
            name: "r".into(),
            capacity: 0,
            rank,
            is_virtual: virt,
            features: vec![],
            federation_owned: false,
        }
    }

    fn grid() -> SlotTable {
        // 2 weeks (teaching, exam), Mon+Sat, 3 blocks/day.
        SlotTable::build(
            3,
            &[1, 6],
            &[
                WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] },
                WeekSpec { kind: WeekKind::Exam, holiday_weekdays: vec![] },
            ],
        )
        .unwrap()
    }

    #[test]
    fn every_predicate_reads_the_grid_not_a_magic_number() {
        let g = grid();
        let plain = room_at(1, false);

        let first = g.flags(g.resolve(0, 1, 0).unwrap());
        let last = g.flags(g.resolve(0, 1, 2).unwrap());
        let sat = g.flags(g.resolve(0, 6, 1).unwrap());
        let exam = g.flags(g.resolve(1, 1, 1).unwrap());

        assert!(SoftParams::MinimizeFirstBlock.applies(first, &plain));
        assert!(!SoftParams::MinimizeFirstBlock.applies(last, &plain));

        assert!(SoftParams::MinimizeLastBlock.applies(last, &plain));
        assert!(!SoftParams::MinimizeLastBlock.applies(first, &plain));

        let sat_rule = SoftParams::MinimizeDayUsage { days: vec![6] };
        assert!(sat_rule.applies(sat, &plain));
        assert!(!sat_rule.applies(first, &plain));

        assert!(SoftParams::MinimizeExamWeek.applies(exam, &plain));
        assert!(!SoftParams::MinimizeExamWeek.applies(first, &plain));

        let rank = SoftParams::MinimizeRoomRank { rank_threshold: 5 };
        assert!(rank.applies(first, &room_at(5, false)));
        assert!(rank.applies(first, &room_at(9, false)));
        assert!(!rank.applies(first, &room_at(4, false)));

        assert!(SoftParams::MinimizeOnline.applies(first, &room_at(1, true)));
        assert!(!SoftParams::MinimizeOnline.applies(first, &plain));
    }

    #[test]
    fn cost_table_sums_applicable_weights() {
        let g = grid();
        let rooms = vec![room_at(1, false), room_at(9, true)];
        let instances = vec![
            SoftInstance {
                id: "first".into(),
                kinds: vec![],
                weight: 2.0,
                params: SoftParams::MinimizeFirstBlock,
            },
            SoftInstance {
                id: "online".into(),
                kinds: vec![],
                weight: 5.0,
                params: SoftParams::MinimizeOnline,
            },
        ];
        let m = SoftModel::build(instances, &g, &rooms, &["lecture".to_string()]);
        let p = m.profile_for_kind("lecture");

        let first = g.resolve(0, 1, 0).unwrap();
        let mid = g.resolve(0, 1, 1).unwrap();

        assert_eq!(m.cost(p, first, RoomIdx(0)), 2.0, "first block, on-site");
        assert_eq!(m.cost(p, first, RoomIdx(1)), 7.0, "first block + online");
        assert_eq!(m.cost(p, mid, RoomIdx(1)), 5.0, "online only");
        assert_eq!(m.cost(p, mid, RoomIdx(0)), 0.0, "neither");
        assert_eq!(m.total_weight, 7.0);
    }

    #[test]
    fn kind_scoping_produces_separate_profiles() {
        let g = grid();
        let rooms = vec![room_at(1, true)];
        let instances = vec![SoftInstance {
            id: "online".into(),
            kinds: vec!["lecture".into()],
            weight: 3.0,
            params: SoftParams::MinimizeOnline,
        }];
        let kinds = vec!["lecture".to_string(), "staff_meeting".to_string()];
        let m = SoftModel::build(instances, &g, &rooms, &kinds);

        let slot = g.resolve(0, 1, 0).unwrap();
        assert_eq!(m.cost(m.profile_for_kind("lecture"), slot, RoomIdx(0)), 3.0);
        assert_eq!(
            m.cost(m.profile_for_kind("staff_meeting"), slot, RoomIdx(0)),
            0.0,
            "a kind outside the instance's scope must cost nothing"
        );
    }

    #[test]
    fn total_is_lexicographic_under_a_derived_penalty() {
        // Any single unplaced session must outrank every soft configuration.
        let hard_penalty = 7.0 * 4.0 + 1.0; // total_weight * placements + 1
        let all_soft_bad = Objective { unplaced: 0, aggregate: 0, soft: 7.0 * 4.0 };
        let one_unplaced = Objective { unplaced: 1, aggregate: 0, soft: 0.0 };
        assert!(one_unplaced.total(hard_penalty) > all_soft_bad.total(hard_penalty));
    }
}
