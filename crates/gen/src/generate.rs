//! Seeded instance generation.
//!
//! Everything here is a pure function of `(params, seed)`. The RNG is consumed
//! **strictly sequentially**, in one pass, for the same reason the search does
//! it: a benchmark instance you cannot regenerate byte-for-byte is not a
//! benchmark, it is an anecdote.
//!
//! The generation seed is deliberately **separate** from the solve seed, so an
//! instance can be held fixed while the search seed varies — and vice versa.

use calendry_solver_core::aggregates::{ShareInstance, ShareWindow};
use calendry_solver_core::ids::{GroupIdx, OfferingIdx, PersonIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{
    ConstraintInstance, ConstraintSet, FixedSpec, Group, Immovable, OfferingSpec, Person,
    PlacementVar, Problem, Room, Unavailability,
};
use calendry_solver_core::rng::Rng;
use calendry_solver_core::slots::{SlotTable, WeekKind, WeekSpec};
use calendry_solver_core::soft::{SoftInstance, SoftParams};

use crate::params::InstanceParams;

/// Tenant-defined kind vocabulary. Three kinds rather than one, so the soft
/// model actually builds distinct profiles at benchmark scale.
const KIND_LECTURE: &str = "lecture";
const KIND_SEMINAR: &str = "seminar";
const KIND_LAB: &str = "lab";

/// Tenant-defined equipment vocabulary.
const FEATURES: [&str; 2] = ["projector", "lab_bench"];

/// Rank assigned to premium rooms. `Room.rank` is ordered higher = more
/// premium/scarce, so `MinimizeRoomRank` thresholds against this.
const PREMIUM_RANK: u32 = 8;
const ORDINARY_RANK: u32 = 2;

pub struct GeneratedInstance {
    pub problem: Problem,
    pub stats: InstanceStats,
}

/// Everything about a generated instance worth reporting next to a timing.
#[derive(Clone, Debug)]
pub struct InstanceStats {
    pub slots: usize,
    pub rooms: usize,
    pub virtual_rooms: usize,
    pub groups: usize,
    pub persons: usize,
    pub offerings: usize,
    pub placements: usize,
    pub fixed: usize,

    pub total_demand_blocks: u64,
    /// Demand as a fraction of the room-slot grid.
    pub room_tightness: f64,
    /// The busiest **group row**: blocks marked against one Group by everything
    /// in its conflict closure, over the length of the term.
    pub max_group_load: f64,
    pub max_lecturer_load: f64,
    /// The binding axis — `max` of the three above. This, not room tightness,
    /// is what decides whether an instance is hard-but-feasible.
    pub saturation: f64,
    pub predicted_saturation: f64,

    pub mean_eligible_rooms: f64,
    pub max_eligible_rooms: usize,
    pub mean_attendees: f64,
    pub max_attendees: usize,

    /// `slots x mean_eligible_rooms` — the full repair enumeration width, and
    /// the direct subject of hypothesis H1.
    pub mean_candidates: f64,
}

/// A cheap structural fingerprint, for asserting that the same seed reproduces
/// the same instance without comparing whole `Problem` values.
pub fn digest(problem: &Problem) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x1000_0000_01b3);
    };

    mix(problem.slots.len() as u64);
    mix(problem.rooms.len() as u64);
    mix(problem.groups.len() as u64);
    mix(problem.persons.len() as u64);
    mix(problem.placements.len() as u64);

    for o in &problem.offerings {
        mix(o.duration_blocks as u64);
        mix(o.eligible_rooms.len() as u64);
        mix(o.attendees.len() as u64);
        mix(o.conflict_groups.len() as u64);
        mix(o.lecturers.iter().map(|l| l.get() as u64 + 1).sum());
        mix(o.veto_slots.count() as u64);
    }
    for f in &problem.fixed {
        mix(f.start.get() as u64);
        mix(f.room.map_or(u64::MAX, |r| r.get() as u64));
        mix(f.duration_blocks as u64);
    }
    h
}

pub fn generate(params: &InstanceParams, seed: u64) -> GeneratedInstance {
    let mut rng = Rng::new(seed);

    let slots = build_grid(params);
    let rooms = build_rooms(params, &mut rng);
    let groups = build_groups(params);
    let group_sizes = group_sizes(params);
    let persons = build_persons(params, &mut rng);

    let (offering_specs, occurrences) =
        build_offerings(params, &rooms, &group_sizes, &mut rng);

    // Locked Sessions carry their Offering link, so `required_session_count`
    // keeps its true domain meaning — the total this Offering needs — and
    // `exact_frequency` counts placements plus locks against it.
    let (placements, fixed) = split_occurrences(
        params,
        &offering_specs,
        &occurrences,
        &rooms,
        &slots,
        &mut rng,
    );

    let constraints = build_constraints(params, &slots);

    let problem = Problem::build(
        slots,
        rooms,
        groups,
        persons,
        offering_specs,
        placements,
        fixed,
        constraints,
    )
    .expect("generated group hierarchy is a forest by construction");

    let stats = measure(params, &problem);
    GeneratedInstance { problem, stats }
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

fn build_grid(params: &InstanceParams) -> SlotTable {
    let weeks: Vec<WeekSpec> = (0..params.weeks)
        .map(|w| {
            // Week kinds come from explicit index lists, never from slicing the
            // tail of the term.
            let kind = if params.exam_weeks.contains(&w) {
                WeekKind::Exam
            } else if params.holiday_weeks.contains(&w) {
                WeekKind::Break
            } else {
                WeekKind::Teaching
            };
            WeekSpec { kind, holiday_weekdays: vec![] }
        })
        .collect();

    SlotTable::build(params.blocks_per_day, &params.active_days, &weeks)
        .expect("preset grids are well-formed")
}

// ---------------------------------------------------------------------------
// Rooms
// ---------------------------------------------------------------------------

/// Physical rooms are split into three capacity tiers matching the three group
/// levels, because that is what actually produces a realistic `eligible_rooms`
/// spread: a cohort lecture fits in few rooms, a seminar fits in most.
///
/// The top two tiers are deliberately **oversupplied** relative to their share
/// of demand (x1.5 and x1.2). A large room substitutes downward for a small
/// group while a small room can never host a large one, so allocating tiers
/// strictly by demand share would leave the cohort tier exactly critical and
/// every other tier slack.
fn tier_shares(params: &InstanceParams) -> [f64; 3] {
    let large = (params.group_level_mix[0] * 1.5).min(0.6);
    let medium = (params.group_level_mix[1] * 1.2).min(0.9 - large);
    [large, medium, 1.0 - large - medium]
}

fn build_rooms(params: &InstanceParams, rng: &mut Rng) -> Vec<Room> {
    let sizes = group_sizes(params);
    let shares = tier_shares(params);
    let n = params.physical_rooms;

    let n_large = ((n as f64) * shares[0]).round() as u32;
    let n_medium = ((n as f64) * shares[1]).round() as u32;

    let mut rooms = Vec::with_capacity(params.rooms() as usize);
    for i in 0..n {
        // 10% headroom over the exact group size: a room sized to the nearest
        // person is not something an institution actually has.
        let capacity = if i < n_large {
            (sizes.cohort as f64 * 1.1).ceil() as u32
        } else if i < n_large + n_medium {
            (sizes.class as f64 * 1.1).ceil() as u32
        } else {
            (sizes.seminar as f64 * 1.1).ceil() as u32
        };

        let premium = (rng.next_u64() % 1000) < (params.premium_ratio * 1000.0) as u64;
        let features: Vec<String> = FEATURES
            .iter()
            .filter(|_| (rng.next_u64() % 1000) < (params.feature_coverage * 1000.0) as u64)
            .map(|f| f.to_string())
            .collect();

        rooms.push(Room {
            id: format!("room-{i}"),
            name: format!("Room {i}"),
            capacity,
            rank: if premium { PREMIUM_RANK } else { ORDINARY_RANK },
            is_virtual: false,
            features,
            federation_owned: false,
        });
    }

    // Online delivery is a virtual Room, not a boolean. Unbounded capacity and
    // every feature, because any session *could* run online — which is what
    // makes MaxOnlineShare and OnlineOnsiteSameDay bind rather than be
    // unreachable.
    for i in 0..params.virtual_rooms {
        rooms.push(Room {
            id: format!("online-{i}"),
            name: format!("Online {i}"),
            capacity: u32::MAX,
            rank: ORDINARY_RANK,
            is_virtual: true,
            features: FEATURES.iter().map(|f| f.to_string()).collect(),
            federation_owned: false,
        });
    }

    rooms
}

// ---------------------------------------------------------------------------
// Groups and people
// ---------------------------------------------------------------------------

struct GroupSizes {
    cohort: u32,
    class: u32,
    seminar: u32,
}

fn group_sizes(params: &InstanceParams) -> GroupSizes {
    let seminar = params.students_per_seminar;
    let class = params.seminars_per_class * seminar;
    GroupSizes { cohort: params.classes_per_cohort * class, class, seminar }
}

/// Index layout: cohorts, then classes, then seminars. Kept explicit so the
/// mapping helpers below are readable rather than arithmetic puzzles.
fn cohort_idx(_params: &InstanceParams, c: u32) -> u32 {
    c
}

fn class_idx(params: &InstanceParams, c: u32, k: u32) -> u32 {
    params.cohorts + c * params.classes_per_cohort + k
}

fn seminar_idx(params: &InstanceParams, c: u32, k: u32, s: u32) -> u32 {
    let classes = params.cohorts * params.classes_per_cohort;
    params.cohorts
        + classes
        + (c * params.classes_per_cohort + k) * params.seminars_per_class
        + s
}

fn build_groups(params: &InstanceParams) -> Vec<Group> {
    let sizes = group_sizes(params);
    let mut groups = Vec::with_capacity(params.group_count() as usize);

    for c in 0..params.cohorts {
        groups.push(Group {
            id: format!("cohort-{c}"),
            parent: None,
            name: format!("Cohort {c}"),
            size: sizes.cohort,
        });
    }
    for c in 0..params.cohorts {
        for k in 0..params.classes_per_cohort {
            groups.push(Group {
                id: format!("class-{c}-{k}"),
                parent: Some(GroupIdx(cohort_idx(params, c))),
                name: format!("Class {c}.{k}"),
                size: sizes.class,
            });
        }
    }
    for c in 0..params.cohorts {
        for k in 0..params.classes_per_cohort {
            for s in 0..params.seminars_per_class {
                groups.push(Group {
                    id: format!("seminar-{c}-{k}-{s}"),
                    parent: Some(GroupIdx(class_idx(params, c, k))),
                    name: format!("Seminar {c}.{k}.{s}"),
                    size: sizes.seminar,
                });
            }
        }
    }
    groups
}

/// Lecturers occupy person indices `0..lecturers`, students follow.
fn build_persons(params: &InstanceParams, rng: &mut Rng) -> Vec<Person> {
    let mut persons =
        Vec::with_capacity(params.lecturers as usize + params.student_count() as usize);

    for i in 0..params.lecturers {
        // A whole research day off, which is the common real shape and is also
        // the widest blackout that still leaves the instance feasible.
        let blackouts = if (rng.next_u64() % 1000) < (params.blackout_ratio * 1000.0) as u64 {
            let day = params.active_days[rng.below(params.active_days.len())];
            vec![Unavailability { days: vec![day], blocks: vec![], weeks: vec![] }]
        } else {
            vec![]
        };
        persons.push(Person {
            id: format!("lecturer-{i}"),
            role_tags: vec!["Lecturer".to_string()],
            groups: vec![],
            blackouts,
        });
    }

    for c in 0..params.cohorts {
        for k in 0..params.classes_per_cohort {
            for s in 0..params.seminars_per_class {
                for n in 0..params.students_per_seminar {
                    let home = GroupIdx(seminar_idx(params, c, k, s));
                    let mut groups = vec![home];

                    // An elective places the student in a seminar under a
                    // DIFFERENT cohort, so the two groups are guaranteed
                    // unrelated in the nesting tree — neither an ancestor nor a
                    // descendant of the other. That is the only configuration
                    // PersonDoubleBooking catches and GroupDoubleBooking
                    // structurally cannot.
                    if params.cohorts > 1
                        && (rng.next_u64() % 1000) < (params.elective_ratio * 1000.0) as u64
                    {
                        let other_c = {
                            let offset = 1 + rng.below((params.cohorts - 1) as usize) as u32;
                            (c + offset) % params.cohorts
                        };
                        let other_k = rng.below(params.classes_per_cohort as usize) as u32;
                        let other_s = rng.below(params.seminars_per_class as usize) as u32;
                        groups.push(GroupIdx(seminar_idx(params, other_c, other_k, other_s)));
                    }

                    persons.push(Person {
                        id: format!("student-{c}-{k}-{s}-{n}"),
                        role_tags: vec!["Student".to_string()],
                        groups,
                        blackouts: vec![],
                    });
                }
            }
        }
    }

    persons
}

// ---------------------------------------------------------------------------
// Offerings
// ---------------------------------------------------------------------------

/// One occurrence of an Offering, before it is split into a placement or a lock.
struct Occurrence {
    offering: u32,
    index: u32,
}

fn build_offerings(
    params: &InstanceParams,
    rooms: &[Room],
    sizes: &GroupSizes,
    rng: &mut Rng,
) -> (Vec<OfferingSpec>, Vec<Occurrence>) {
    let mut specs = Vec::with_capacity(params.offerings as usize);
    let mut occurrences =
        Vec::with_capacity(params.total_occurrences() as usize);

    let cum0 = params.group_level_mix[0];
    let cum1 = cum0 + params.group_level_mix[1];

    for i in 0..params.offerings {
        // Cohorts are assigned **round-robin**, not uniformly at random.
        //
        // The cohort row is the binding axis (every descendant's Session marks
        // it), so random assignment makes the *busiest* cohort — not the mean —
        // decide feasibility, and the max of N draws grows with N. That made
        // measured saturation exceed the closed-form prediction by 1.3x at
        // school scale and 1.55x at university scale, so a single preset
        // calibration could not hold across the range. Balancing the binding
        // axis is also the more realistic shape: a curriculum is planned evenly
        // across year groups, not sampled with replacement.
        //
        // Class and seminar choice *within* the cohort stays random, so the
        // sub-structure keeps a realistic spread.
        let c = i % params.cohorts;
        let roll = (rng.next_u64() % 1000) as f64 / 1000.0;
        let (group, size, kind) = if roll < cum0 {
            (cohort_idx(params, c), sizes.cohort, KIND_LECTURE)
        } else if roll < cum1 {
            let k = rng.below(params.classes_per_cohort as usize) as u32;
            (class_idx(params, c, k), sizes.class, KIND_SEMINAR)
        } else {
            let k = rng.below(params.classes_per_cohort as usize) as u32;
            let s = rng.below(params.seminars_per_class as usize) as u32;
            (seminar_idx(params, c, k, s), sizes.seminar, KIND_LAB)
        };

        let required_feature =
            if (rng.next_u64() % 1000) < (params.feature_demand_ratio * 1000.0) as u64 {
                Some(FEATURES[rng.below(FEATURES.len())])
            } else {
                None
            };

        let eligible_rooms: Vec<RoomIdx> = rooms
            .iter()
            .enumerate()
            .filter(|(_, r)| r.capacity >= size)
            .filter(|(_, r)| {
                required_feature.is_none_or(|f| r.features.iter().any(|rf| rf == f))
            })
            .map(|(n, _)| RoomIdx(n as u32))
            .collect();

        let span = params.duration_blocks.1 - params.duration_blocks.0 + 1;
        let duration = params.duration_blocks.0 + rng.below(span as usize) as u32;

        // Round-robin over lecturers, so load is even by construction and the
        // lecturer axis is genuinely contended rather than accidentally slack.
        let lecturer = PersonIdx(i % params.lecturers);

        specs.push(OfferingSpec {
            id: format!("offering-{i}"),
            kind: kind.to_string(),
            required_session_count: params.sessions_per_offering,
            duration_blocks: duration,
            lecturers: vec![lecturer],
            groups: vec![GroupIdx(group)],
            participants: vec![],
            eligible_rooms,
        });

        for n in 0..params.sessions_per_offering {
            occurrences.push(Occurrence { offering: i, index: n });
        }
    }

    (specs, occurrences)
}

// ---------------------------------------------------------------------------
// Placements vs. immovable occupancy
// ---------------------------------------------------------------------------

/// Split occurrences into solver-placed variables and locked Sessions.
///
/// Locked ones are **subtracted from** the placements rather than added on top,
/// so total demand — and therefore tightness — is unchanged by `locked_ratio`.
///
/// They are positioned by first fit from a random start offset, against a
/// scratch room + lecturer occupancy. Room and lecturer only: the caller's
/// "warn and allow" editing UX can genuinely hand the solver input that already
/// violates a group or person constraint, and the solver must tolerate it, so
/// generating a perfectly consistent lock set would be less realistic, not more.
fn split_occurrences(
    params: &InstanceParams,
    specs: &[OfferingSpec],
    occurrences: &[Occurrence],
    rooms: &[Room],
    slots: &SlotTable,
    rng: &mut Rng,
) -> (Vec<PlacementVar>, Vec<FixedSpec>) {
    let n_slots = slots.len();
    let mut room_busy = vec![false; n_slots * rooms.len().max(1)];
    let mut lecturer_busy = vec![false; n_slots * params.lecturers.max(1) as usize];

    let mut placements = Vec::with_capacity(occurrences.len());
    let mut fixed = Vec::new();

    for occ in occurrences {
        let spec = &specs[occ.offering as usize];
        let lock = (rng.next_u64() % 1000) < (params.locked_ratio * 1000.0) as u64;

        if !lock {
            placements.push(PlacementVar {
                offering: OfferingIdx(occ.offering),
                occurrence: occ.index,
                existing_session_id: None,
            });
            continue;
        }

        let start_at = rng.below(n_slots);
        let mut placed = None;
        'scan: for step in 0..n_slots {
            let s = SlotIdx(((start_at + step) % n_slots) as u32);
            let Some(span) = slots.span(s, spec.duration_blocks) else {
                continue;
            };
            let lect = spec.lecturers[0].get();
            if span
                .iter()
                .any(|x| lecturer_busy[x.get() * params.lecturers as usize + lect])
            {
                continue;
            }
            for &room in &spec.eligible_rooms {
                if span
                    .iter()
                    .any(|x| room_busy[x.get() * rooms.len() + room.get()])
                {
                    continue;
                }
                for x in &span {
                    room_busy[x.get() * rooms.len() + room.get()] = true;
                    lecturer_busy[x.get() * params.lecturers as usize + lect] = true;
                }
                placed = Some((s, room));
                break 'scan;
            }
        }

        match placed {
            Some((start, room)) => fixed.push(FixedSpec {
                session_id: format!("{}#{}-locked", spec.id, occ.index),
                // A locked Session still realizes its Offering, so it counts
                // toward that Offering's required frequency.
                offering: Some(OfferingIdx(occ.offering)),
                kind: spec.kind.clone(),
                room: Some(room),
                start,
                duration_blocks: spec.duration_blocks,
                lecturers: spec.lecturers.clone(),
                groups: spec.groups.clone(),
                persons: vec![],
                reason: Immovable::Locked,
            }),
            // Nowhere left to lock it: keep it as a placement rather than
            // dropping demand on the floor, which would skew tightness.
            None => placements.push(PlacementVar {
                offering: OfferingIdx(occ.offering),
                occurrence: occ.index,
                existing_session_id: None,
            }),
        }
    }

    (placements, fixed)
}

// ---------------------------------------------------------------------------
// Constraints
// ---------------------------------------------------------------------------

fn build_constraints(params: &InstanceParams, slots: &SlotTable) -> ConstraintSet {
    let all = |id: &str| ConstraintInstance { id: id.to_string(), kinds: vec![] };
    let w = params.soft_weight;

    // A real weekday the tenant teaches on, chosen as data. The solver reads it
    // out of the parameters; nothing derives a day from slot arithmetic.
    let discouraged_day = *slots
        .active_days()
        .last()
        .expect("grid has at least one active day");

    let soft = vec![
        SoftInstance {
            id: "soft-first-block".into(),
            kinds: vec![],
            weight: w,
            params: SoftParams::MinimizeFirstBlock,
        },
        SoftInstance {
            id: "soft-last-block".into(),
            kinds: vec![],
            weight: w,
            params: SoftParams::MinimizeLastBlock,
        },
        SoftInstance {
            id: "soft-day-usage".into(),
            kinds: vec![],
            weight: 3.0 * w,
            params: SoftParams::MinimizeDayUsage { days: vec![discouraged_day] },
        },
        SoftInstance {
            id: "soft-room-rank".into(),
            kinds: vec![],
            weight: 2.0 * w,
            params: SoftParams::MinimizeRoomRank { rank_threshold: PREMIUM_RANK },
        },
        SoftInstance {
            id: "soft-exam-week".into(),
            kinds: vec![],
            weight: 5.0 * w,
            params: SoftParams::MinimizeExamWeek,
        },
        SoftInstance {
            id: "soft-online".into(),
            kinds: vec![],
            weight: 2.0 * w,
            // Scoped to lectures only, so the soft model builds more than one
            // profile and the per-kind table split is exercised at scale.
            params: SoftParams::MinimizeOnline,
        },
    ];

    ConstraintSet {
        room_double_booking: vec![all("c-room")],
        lecturer_double_booking: vec![all("c-lecturer")],
        group_double_booking: vec![all("c-group")],
        person_double_booking: vec![all("c-person")],
        exact_frequency: vec![all("c-frequency")],
        lecturer_veto: vec![all("c-veto")],
        online_onsite_same_day: vec![ConstraintInstance {
            id: "c-day-mix".into(),
            kinds: vec![KIND_LECTURE.into(), KIND_SEMINAR.into(), KIND_LAB.into()],
        }],
        max_online_share: params
            .max_online_share
            .map(|r| ShareInstance {
                id: "c-share".into(),
                kinds: vec![],
                max_ratio: r,
                window: ShareWindow::PerTerm,
            })
            .into_iter()
            .collect(),
        soft,
    }
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

/// Per-axis saturation.
///
/// Room tightness is **not** the binding quantity, which is what the first
/// calibration pass got wrong. A Group can only be in one place at a time, and
/// conflict propagation marks a parent Group for every Session of every
/// descendant — so a cohort's row accumulates the demand of its entire subtree
/// while a room's row accumulates only what is actually placed in that room.
/// For any realistic hierarchy the group axis saturates first, by a wide
/// margin, and calibrating against rooms produces instances where construction
/// cannot place most Sessions at all.
fn measure(params: &InstanceParams, problem: &Problem) -> InstanceStats {
    let n_slots = problem.slots.len();
    let n_rooms = problem.rooms.len();

    let mut demand: u64 = 0;
    let mut group_blocks = vec![0u64; problem.groups.len().max(1)];
    let mut lecturer_blocks = vec![0u64; problem.persons.len().max(1)];

    let mut charge = |d: u32, groups: &[calendry_solver_core::ids::GroupIdx],
                      lecturers: &[PersonIdx]| {
        for g in groups {
            group_blocks[g.get()] += d as u64;
        }
        for l in lecturers {
            lecturer_blocks[l.get()] += d as u64;
        }
    };

    for p in problem.placement_ids() {
        let o = problem.offering_of(p);
        demand += o.duration_blocks as u64;
        charge(o.duration_blocks, &o.conflict_groups, &o.lecturers);
    }
    for f in &problem.fixed {
        demand += f.duration_blocks as u64;
        charge(f.duration_blocks, &f.conflict_groups, &f.lecturers);
    }

    let slots_f = n_slots as f64;
    let room_tightness = demand as f64 / (slots_f * n_rooms as f64);
    let max_group_load =
        group_blocks.iter().copied().max().unwrap_or(0) as f64 / slots_f;
    let max_lecturer_load =
        lecturer_blocks.iter().copied().max().unwrap_or(0) as f64 / slots_f;
    let saturation = room_tightness.max(max_group_load).max(max_lecturer_load);

    let n_off = problem.offerings.len().max(1) as f64;
    let eligible: Vec<usize> = problem.offerings.iter().map(|o| o.eligible_rooms.len()).collect();
    let attendees: Vec<usize> = problem.offerings.iter().map(|o| o.attendees.len()).collect();

    let mean_eligible = eligible.iter().sum::<usize>() as f64 / n_off;

    InstanceStats {
        slots: n_slots,
        rooms: n_rooms,
        virtual_rooms: problem.rooms.iter().filter(|r| r.is_virtual).count(),
        groups: problem.groups.len(),
        persons: problem.persons.len(),
        offerings: problem.offerings.len(),
        placements: problem.placements.len(),
        fixed: problem.fixed.len(),

        total_demand_blocks: demand,
        room_tightness,
        max_group_load,
        max_lecturer_load,
        saturation,
        predicted_saturation: params.predicted_saturation(),

        mean_eligible_rooms: mean_eligible,
        max_eligible_rooms: eligible.iter().copied().max().unwrap_or(0),
        mean_attendees: attendees.iter().sum::<usize>() as f64 / n_off,
        max_attendees: attendees.iter().copied().max().unwrap_or(0),

        mean_candidates: n_slots as f64 * mean_eligible,
    }
}
