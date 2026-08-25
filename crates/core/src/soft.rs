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
    /// DEPRECATED on the wire, superseded by `MinimizeBlockUsage { first: true }`.
    /// Kept because a peer on the old schema can still send it.
    MinimizeFirstBlock,
    /// DEPRECATED on the wire, superseded by `MinimizeBlockUsage { last: true }`.
    MinimizeLastBlock,
    /// Penalize the listed block positions. Does for the BLOCK axis what
    /// `MinimizeDayUsage` did for the day axis.
    ///
    /// `blocks` are ABSOLUTE 0-based indices; `first`/`last` are RELATIVE and
    /// track `blocks_per_day`. Both exist because they mean different things: a
    /// grid that grows from 6 blocks to 8 leaves "avoid index 5" pointing at
    /// mid-afternoon, while `last` still means the last block. Absolute indices
    /// alone would lose an intent the deprecated variants could express.
    ///
    /// An index at or beyond `blocks_per_day` simply never matches — inert
    /// rather than an error, because the app lets a grid shrink under its own
    /// configuration and a stale index must not fail a whole run.
    MinimizeBlockUsage { blocks: Vec<u32>, first: bool, last: bool },
    /// Penalize the listed ISO weekdays (1 = Monday). Generalizes the
    /// prototype's hardcoded "minimize Saturday": with tenant-configured
    /// `active_days`, Saturday is not structurally special.
    MinimizeDayUsage { days: Vec<u32> },
    /// `Room.rank` is ordered **higher = more premium/scarce**.
    ///
    /// `invert` selects which side of the threshold is penalized:
    ///   false — `rank >= rank_threshold`, sparing the best rooms
    ///   true  — `rank <= rank_threshold`, preferring them
    ///
    /// Both are real policies. An institution may want its best halls kept free
    /// for events, or may want them USED for teaching rather than standing empty
    /// while lessons go into the cheap rooms.
    ///
    /// A flag rather than a second variant, mirroring MinimizeBlockUsage
    /// replacing MinimizeFirstBlock/MinimizeLastBlock: two directions of one
    /// axis over one field. Two variants would also be separately instantiable,
    /// so a tenant could enable both and penalize rooms from both ends at once.
    MinimizeRoomRank { rank_threshold: u32, invert: bool },
    MinimizeExamWeek,
    MinimizeOnline,
}

impl SoftParams {
    pub fn type_name(&self) -> &'static str {
        match self {
            SoftParams::MinimizeFirstBlock => "MinimizeFirstBlock",
            SoftParams::MinimizeLastBlock => "MinimizeLastBlock",
            SoftParams::MinimizeBlockUsage { .. } => "MinimizeBlockUsage",
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
            SoftParams::MinimizeBlockUsage { blocks, first, last } => {
                (*first && f.is_first_block)
                    || (*last && f.is_last_block)
                    || blocks.contains(&f.block)
            }
            SoftParams::MinimizeDayUsage { days } => days.contains(&f.iso_weekday),
            SoftParams::MinimizeRoomRank { rank_threshold, invert } => if *invert {
                room.rank <= *rank_threshold
            } else {
                room.rank >= *rank_threshold
            },
            SoftParams::MinimizeExamWeek => f.week_kind == WeekKind::Exam,
            SoftParams::MinimizeOnline => room.is_virtual,
        }
    }

    /// How STRONGLY this instance penalizes `room`, in `0.0..=1.0`.
    ///
    /// `applies` decides whether a room is penalized at all; this decides by how
    /// much, and returns 0.0 exactly when `applies` is false — so the two can
    /// never disagree about which rooms are affected.
    ///
    /// ONLY `MinimizeRoomRank` GRADES. Every other type is a property a slot
    /// either has or does not ("this is the last block", "this week is an exam
    /// week"), with no meaningful notion of degree, so they return 1.0 and cost
    /// exactly their weight as before.
    ///
    /// WHY THE RESULT IS CAPPED AT 1.0, WHICH IS NOT A STYLE CHOICE.
    /// `Problem` derives `hard_penalty = sum(weights) * placements + 1` and
    /// relies on it dominating "any achievable soft total", which is what makes
    /// the scalar objective order lexicographically — hard constraints first,
    /// soft ones only as a tie-break. That bound holds precisely because each
    /// instance contributes AT MOST its weight per placement. A raw distance
    /// multiplier (rank 10 costing ten times rank 1) would break it, and a soft
    /// preference could then outrank a hard constraint. Normalising against the
    /// room set's own rank span keeps the gradient while preserving the bound,
    /// and also keeps this rule's weight comparable to every other soft rule the
    /// tenant has tuned against it.
    #[inline]
    pub fn severity(&self, f: &SlotFlags, room: &Room, ranks: RankSpan) -> f64 {
        if !self.applies(f, room) {
            return 0.0;
        }

        match self {
            SoftParams::MinimizeRoomRank { rank_threshold, invert } => {
                // Distance PAST the threshold, and the largest such distance any
                // room in this problem reaches. `+1` on both so a room exactly at
                // the threshold still costs something — it is penalized, just
                // least — and so the span being zero cannot divide by zero.
                let (distance, span) = if *invert {
                    (rank_threshold.saturating_sub(room.rank),
                     rank_threshold.saturating_sub(ranks.min))
                } else {
                    (room.rank.saturating_sub(*rank_threshold),
                     ranks.max.saturating_sub(*rank_threshold))
                };

                (distance as f64 + 1.0) / (span as f64 + 1.0)
            }
            _ => 1.0,
        }
    }
}

/// The rank extremes of a problem's rooms, so a graded penalty can be
/// normalised against the building it is actually scheduling.
///
/// Derived from the room set rather than configured: "how premium is premium"
/// is a property of the estate, and a tenant-supplied maximum would be a second
/// number to keep in step with the rooms themselves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RankSpan {
    pub min: u32,
    pub max: u32,
}

impl RankSpan {
    pub fn of(rooms: &[Room]) -> Self {
        Self {
            min: rooms.iter().map(|r| r.rank).min().unwrap_or(0),
            max: rooms.iter().map(|r| r.rank).max().unwrap_or(0),
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
        let ranks = RankSpan::of(rooms);

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
                            // `severity` is 0.0 when the instance does not apply,
                            // so this is the same gate as before plus a magnitude.
                            c += instances[m].weight
                                * instances[m].params.severity(f, room, ranks);
                        }
                        t[slot.get() * n_rooms + r] = c;
                    }
                }
                t
            })
            .collect();

        Self {
            instances,
            profiles,
            tables,
            profile_of_kind,
            n_rooms,
            total_weight,
        }
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
    /// Mixed `(group, day)` cells, already multiplied by the configured weight.
    ///
    /// SEPARATE FROM `soft`, and the split is about how each is maintained
    /// rather than about what they mean. `soft` is a per-placement unary cost
    /// the search accumulates as a delta; this is read whole off the counters,
    /// like `aggregate`, because a mixed cell belongs to no single placement.
    /// Adding it into `soft` would mix an accumulated total with an assigned
    /// one in one field, and the drift assertion that keeps the incremental
    /// objective honest could no longer tell the two apart.
    ///
    /// Stored pre-multiplied so `total()` keeps its signature and every caller
    /// does not have to carry the weight around.
    pub day_mix_cost: f64,
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
        self.hard() as f64 * hard_penalty + self.soft + self.day_mix_cost
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
    fn block_usage_selects_by_index_and_by_relative_position() {
        // 3 blocks/day, so index 0 is first and index 2 is last.
        let g = grid();
        let plain = room_at(1, false);
        let at = |b: u32| g.flags(g.resolve(0, 1, b).unwrap());

        // Absolute index, and nothing else.
        let middle = SoftParams::MinimizeBlockUsage { blocks: vec![1], first: false, last: false };
        assert!(!middle.applies(at(0), &plain));
        assert!(middle.applies(at(1), &plain));
        assert!(!middle.applies(at(2), &plain));

        // Relative flags, with no indices at all.
        let ends = SoftParams::MinimizeBlockUsage { blocks: vec![], first: true, last: true };
        assert!(ends.applies(at(0), &plain));
        assert!(!ends.applies(at(1), &plain));
        assert!(ends.applies(at(2), &plain));

        // The two compose rather than override each other.
        let both = SoftParams::MinimizeBlockUsage { blocks: vec![1], first: true, last: false };
        assert!(both.applies(at(0), &plain));
        assert!(both.applies(at(1), &plain));
        assert!(!both.applies(at(2), &plain));
    }

    #[test]
    fn block_usage_reproduces_the_two_deprecated_variants() {
        // The supersession claim, asserted rather than assumed: anything the old
        // messages could express, the new one expresses identically. If this
        // drifts, senders migrating off MinimizeFirstBlock/MinimizeLastBlock get
        // a different timetable for what they were told is the same rule.
        let g = grid();
        let plain = room_at(1, false);

        for b in 0..3 {
            let f = g.flags(g.resolve(0, 1, b).unwrap());

            assert_eq!(
                SoftParams::MinimizeFirstBlock.applies(f, &plain),
                SoftParams::MinimizeBlockUsage { blocks: vec![], first: true, last: false }
                    .applies(f, &plain),
                "first-block parity at block {b}"
            );
            assert_eq!(
                SoftParams::MinimizeLastBlock.applies(f, &plain),
                SoftParams::MinimizeBlockUsage { blocks: vec![], first: false, last: true }
                    .applies(f, &plain),
                "last-block parity at block {b}"
            );
        }
    }

    #[test]
    fn a_block_index_past_the_end_of_the_day_is_inert() {
        // The grid has 3 blocks; index 9 is what a rule looks like after the
        // tenant shrank the day under it. Inert, never a panic and never a
        // match — the solver tolerates input the app's warn-and-allow UX can
        // produce.
        let g = grid();
        let plain = room_at(1, false);
        let stale = SoftParams::MinimizeBlockUsage { blocks: vec![9], first: false, last: false };

        for b in 0..3 {
            assert!(!stale.applies(g.flags(g.resolve(0, 1, b).unwrap()), &plain));
        }
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

        let rank = SoftParams::MinimizeRoomRank { rank_threshold: 5, invert: false };
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
        let all_soft_bad = Objective {
            unplaced: 0,
            aggregate: 0,
            soft: 7.0 * 4.0,
            day_mix_cost: 0.0,
        };
        let one_unplaced = Objective {
            unplaced: 1,
            aggregate: 0,
            soft: 0.0,
            day_mix_cost: 0.0,
        };
        assert!(one_unplaced.total(hard_penalty) > all_soft_bad.total(hard_penalty));
    }

    #[test]
    fn the_day_mix_term_stays_under_the_hard_penalty_too() {
        /*
         * The bound day-mix needs is NOT `weight * placements` — a mixed cell
         * belongs to a (group, day), and one placement can create several while
         * two are needed before any exists. `Problem::build` multiplies by the
         * CELL COUNT for that reason, and this pins the property that
         * multiplier exists to protect: even with every cell mixed, one unplaced
         * Session still outranks the lot.
         */
        let cells = 30.0; // 6 groups x 5 days
        let day_mix_weight = 5.0;
        let hard_penalty = 7.0 * 4.0 + day_mix_weight * cells + 1.0;

        let everything_mixed = Objective {
            unplaced: 0,
            aggregate: 0,
            soft: 7.0 * 4.0,
            day_mix_cost: day_mix_weight * cells,
        };
        let one_unplaced = Objective {
            unplaced: 1,
            aggregate: 0,
            soft: 0.0,
            day_mix_cost: 0.0,
        };

        assert!(one_unplaced.total(hard_penalty) > everything_mixed.total(hard_penalty));
    }
}
