//! Seeded instance generation.
//!
//! Everything here is a pure function of `(params, seed)`. The RNG is consumed
//! **strictly sequentially**, in one pass, for the same reason the search does
//! it: a benchmark instance you cannot regenerate byte-for-byte is not a
//! benchmark, it is an anecdote.
//!
//! The generation seed is deliberately **separate** from the solve seed, so an
//! instance can be held fixed while the search seed varies — and vice versa.

use calendry_solver_core::aggregates::{DayMixInstance, ShareInstance, ShareWindow};
use calendry_solver_core::ids::{GroupIdx, OfferingIdx, PersonIdx, RoomIdx, SlotIdx};
use calendry_solver_core::preferences::{Preference, PreferenceInstance};
use calendry_solver_core::problem::{
    ConstraintInstance, ConstraintSet, FixedSpec, Group, Immovable, OfferingSpec, Person,
    PlacementVar, Problem, ProblemSpec, Room, SchedulingPattern, Unavailability,
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
const KIND_ELECTIVE: &str = "elective";

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
    /// Demand of a **mutually-conflicting set** of Offerings, over the term.
    ///
    /// A load metric cannot see this class of infeasibility, and one silently
    /// certified impossible instances as "in band" before slice 6a. Group,
    /// lecturer and room load each ask "how busy is one row"; here every row is
    /// lightly loaded and the failure is that the attendee *sets pairwise
    /// intersect*, so no two members can ever share a slot. That is a
    /// graph-colouring bound, not a load bound. Above 1.0 the instance is
    /// **provably** unplaceable.
    pub person_clique_load: f64,
    pub person_clique_size: usize,
    /// The binding axis — `max` of everything above. This, not room tightness,
    /// is what decides whether an instance is hard-but-feasible.
    pub saturation: f64,
    pub predicted_saturation: f64,
    /// How far the closed form is from the measurement, as a signed fraction of
    /// the measurement: `(predicted - saturation) / saturation`.
    ///
    /// `InstanceStats` carried both figures, and the calibration test asserted
    /// each *independently* lay in the target band — but never that they
    /// **agree**. Predicted 0.56 against measured 0.74 passed green while the
    /// model was badly wrong, and the only thing watching for that was a human
    /// reading the harness's printout.
    ///
    /// This is not hypothetical. The model has already been wrong by 1.28x at
    /// school scale and 1.55x at university scale, and no single calibration
    /// held across the range until the generator was changed. That is why the
    /// agreement is now a field with a test against it.
    pub prediction_error: f64,

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

    let (offering_specs, occurrences) = build_offerings(params, &rooms, &group_sizes, &mut rng);

    // Locked Sessions carry their Offering link, so `required_session_count`
    // keeps its true domain meaning — the total this Offering needs — and
    // `exact_frequency` counts placements plus locks against it.
    let (placements, fixed) =
        split_occurrences(params, &offering_specs, &occurrences, &rooms, &slots, &mut rng);

    let constraints = build_constraints(params, &slots);

    // Every generated Offering is in scope: an instance is built whole, so there
    // is no out-of-scope region for a lock policy to protect. The locked
    // Sessions `split_occurrences` produces exercise the *frequency* accounting,
    // not the scope gate.
    let problem = Problem::build(ProblemSpec {
        rooms,
        groups,
        persons,
        offerings: offering_specs,
        placements,
        fixed,
        constraints,
        ..ProblemSpec::new(slots)
    })
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
            .map(ToString::to_string)
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
            features: FEATURES.iter().map(ToString::to_string).collect(),
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
    params.cohorts + classes + (c * params.classes_per_cohort + k) * params.seminars_per_class + s
}

/// Elective groups occupy the indices after every Seminar.
fn elective_idx(params: &InstanceParams, e: u32) -> u32 {
    params.cohorts + params.cohorts * params.classes_per_cohort + params.seminar_count() + e
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
            // Generated instances declare no Group blackouts. Drawing them
            // randomly here would change every preset's output, and the presets
            // are the benchmark baseline — realism for this field belongs behind
            // a gated parameter, the way per-person preferences were added.
            blackouts: vec![],
        });
    }
    for c in 0..params.cohorts {
        for k in 0..params.classes_per_cohort {
            groups.push(Group {
                id: format!("class-{c}-{k}"),
                parent: Some(GroupIdx(cohort_idx(params, c))),
                name: format!("Class {c}.{k}"),
                size: sizes.class,
                blackouts: vec![],
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
                    blackouts: vec![],
                });
            }
        }
    }

    // Elective groups are ROOTS. Parenting them under a Cohort would put every
    // enrolled student into that Cohort's subtree, making them an attendee of
    // its cohort-wide lectures — which is exactly what made these instances
    // infeasible before. As roots they stay tree-unrelated to the student's home
    // Seminar, so PersonDoubleBooking still has real work to do, without
    // welding two Cohorts' lecture series together.
    for e in 0..params.elective_groups() {
        groups.push(Group {
            id: format!("elective-{e}"),
            parent: None,
            name: format!("Elective {e}"),
            size: sizes.class,
            blackouts: vec![],
        });
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
        // A preferred day plus, half the time, a preferred block — the two
        // shapes the additive rule distinguishes, so a generated instance
        // exercises both divisors rather than only the 2-axis one.
        //
        // Deliberately NOT the blackout day: a preference the lecturer is
        // already unavailable for is inert, and an instance made of inert data
        // measures nothing.
        //
        // GATED so that no RNG is consumed when the ratio is 0.0. Drawing
        // unconditionally would shift every subsequent draw and change the
        // preset instances themselves — the 27,136-Session `large-university`
        // in `docs/PERFORMANCE.md` became a 27,134-Session one, which is a
        // silently different benchmark reporting the same name.
        let preferred = if params.preference_ratio > 0.0
            && (rng.next_u64() % 1000) < (params.preference_ratio * 1000.0) as u64
        {
            let day = params.active_days[rng.below(params.active_days.len())];
            let blocks = if rng.below(2) == 0 {
                vec![]
            } else {
                vec![rng.below(params.blocks_per_day as usize) as u32]
            };
            let multiplier = match rng.below(4) {
                0 => Some(0.5),
                1 => Some(2.0),
                _ => None,
            };
            Some(Preference {
                days: vec![day],
                blocks,
                room_features: vec![],
                weight_multiplier: multiplier,
            })
        } else {
            None
        };
        persons.push(Person {
            id: format!("lecturer-{i}"),
            role_tags: vec!["Lecturer".to_string()],
            groups: vec![],
            blackouts,
            preferred,
        });
    }

    for c in 0..params.cohorts {
        for k in 0..params.classes_per_cohort {
            for s in 0..params.seminars_per_class {
                for n in 0..params.students_per_seminar {
                    let home = GroupIdx(seminar_idx(params, c, k, s));
                    let mut groups = vec![home];

                    // An elective adds a ROOT-level group, unrelated to the
                    // home Seminar in the nesting tree — neither an ancestor
                    // nor a descendant. That is the only configuration
                    // PersonDoubleBooking catches and GroupDoubleBooking
                    // structurally cannot, so the type still earns its keep.
                    //
                    // Crucially it does NOT enrol the student in another
                    // Cohort's subtree, which would make them an attendee of
                    // that Cohort's lectures too.
                    let n_elective = params.elective_groups();
                    if n_elective > 0
                        && (rng.next_u64() % 1000) < (params.elective_ratio * 1000.0) as u64
                    {
                        let e = rng.below(n_elective as usize) as u32;
                        groups.push(GroupIdx(elective_idx(params, e)));
                    }

                    persons.push(Person {
                        id: format!("student-{c}-{k}-{s}-{n}"),
                        role_tags: vec!["Student".to_string()],
                        groups,
                        blackouts: vec![],
                        // Students never state one: the counted set is
                        // lecturers only, so student preferences would be
                        // generated data nothing reads.
                        preferred: None,
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
    let mut occurrences = Vec::with_capacity(params.total_occurrences() as usize);

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
            .filter(|(_, r)| required_feature.is_none_or(|f| r.features.iter().any(|rf| rf == f)))
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
            scheduling_pattern: SchedulingPattern::Unspecified,
        });

        for n in 0..params.sessions_per_offering {
            occurrences.push(Occurrence { offering: i, index: n });
        }
    }

    // Elective Offerings, attached to the root-level elective groups.
    //
    // These are what an elective actually is: its own teaching, with its own
    // cohort of students drawn from across the institution. They conflict with
    // their attendees' home Sessions — which is real, and is what
    // PersonDoubleBooking exists for — but they do not couple two Cohorts'
    // lecture series to each other.
    let n_elective = params.elective_groups();
    for e in 0..n_elective {
        for j in 0..params.elective_offerings_per_group {
            let i = specs.len() as u32;
            let group = elective_idx(params, e);

            let required_feature =
                if (rng.next_u64() % 1000) < (params.feature_demand_ratio * 1000.0) as u64 {
                    Some(FEATURES[rng.below(FEATURES.len())])
                } else {
                    None
                };
            let eligible_rooms: Vec<RoomIdx> = rooms
                .iter()
                .enumerate()
                .filter(|(_, r)| r.capacity >= sizes.class)
                .filter(|(_, r)| {
                    required_feature.is_none_or(|f| r.features.iter().any(|rf| rf == f))
                })
                .map(|(n, _)| RoomIdx(n as u32))
                .collect();

            let span = params.duration_blocks.1 - params.duration_blocks.0 + 1;
            let duration = params.duration_blocks.0 + rng.below(span as usize) as u32;
            let lecturer = PersonIdx(i % params.lecturers);

            specs.push(OfferingSpec {
                id: format!("elective-offering-{e}-{j}"),
                kind: KIND_ELECTIVE.to_string(),
                required_session_count: params.sessions_per_offering,
                duration_blocks: duration,
                lecturers: vec![lecturer],
                groups: vec![GroupIdx(group)],
                participants: vec![],
                eligible_rooms,
                scheduling_pattern: SchedulingPattern::Unspecified,
            });

            for n in 0..params.sessions_per_offering {
                occurrences.push(Occurrence { offering: i, index: n });
            }
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
                original: None,
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
                original: None,
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
            params: SoftParams::MinimizeRoomRank { rank_threshold: PREMIUM_RANK, invert: false },
        },
        SoftInstance {
            id: "soft-exam-week".into(),
            kinds: vec![],
            weight: 5.0 * w,
            params: SoftParams::MinimizeExamWeek { invert: false },
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
        // OFF in generated instances, unlike `lecturer_veto` above: no
        // generated Group declares a blackout, so the mask would be empty and
        // the rule inert. An enabled rule that can never fire is the
        // `lecturer_veto` shape, and the benchmark baseline is the last place
        // to put one.
        group_veto: Vec::new(),
        // Weight 5 mirrors the app catalogue's `defaultWeight`, so generated
        // benchmark instances price a mixed day the way a real tenant does.
        online_onsite_same_day: vec![DayMixInstance {
            id: "c-day-mix".into(),
            kinds: vec![
                KIND_LECTURE.into(),
                KIND_SEMINAR.into(),
                KIND_LAB.into(),
                KIND_ELECTIVE.into(),
            ],
            weight: 5.0,
        }],
        // Configured only when the generator was asked for preferences, so the
        // presets keep exactly the objective `docs/PERFORMANCE.md` measured.
        person_preference_fit: if params.preference_ratio > 0.0 {
            vec![PreferenceInstance { id: "c-pref".into(), kinds: vec![], weight: 2.0 * w }]
        } else {
            Vec::new()
        },
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
        // Not part of any calibrated preset — `docs/PERFORMANCE.md` has no
        // measurement of any of these three to keep stable, unlike the types
        // above.
        compactness: Vec::new(),
        distributed_pattern_adherence: Vec::new(),
        block_pattern_adherence: Vec::new(),
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

    let mut charge = |d: u32, groups: &[GroupIdx], lecturers: &[PersonIdx]| {
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
    let max_group_load = group_blocks.iter().copied().max().unwrap_or(0) as f64 / slots_f;
    let max_lecturer_load = lecturer_blocks.iter().copied().max().unwrap_or(0) as f64 / slots_f;

    let (person_clique_size, clique_blocks) = person_clique(problem);
    let person_clique_load = clique_blocks as f64 / slots_f;

    let saturation = room_tightness
        .max(max_group_load)
        .max(max_lecturer_load)
        .max(person_clique_load);

    let n_off = problem.offerings.len().max(1) as f64;
    let eligible: Vec<usize> = problem
        .offerings
        .iter()
        .map(|o| o.eligible_rooms.len())
        .collect();
    let attendees: Vec<usize> = problem
        .offerings
        .iter()
        .map(|o| o.attendees.len())
        .collect();

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
        person_clique_load,
        person_clique_size,
        saturation,
        predicted_saturation: params.predicted_saturation(),
        prediction_error: if saturation > 0.0 {
            (params.predicted_saturation() - saturation) / saturation
        } else {
            0.0
        },

        mean_eligible_rooms: mean_eligible,
        max_eligible_rooms: eligible.iter().copied().max().unwrap_or(0),
        mean_attendees: attendees.iter().sum::<usize>() as f64 / n_off,
        max_attendees: attendees.iter().copied().max().unwrap_or(0),

        mean_candidates: n_slots as f64 * mean_eligible,
    }
}

/// A greedy lower bound on the largest set of Offerings that pairwise share an
/// attendee, and the block-demand of every Session realizing them.
///
/// # Why a clique and not a load figure
///
/// Two Offerings sharing even one attendee can never occupy the same slot under
/// `PersonDoubleBooking`. A set that pairwise conflicts therefore needs one
/// distinct slot per Session, so `sum(sessions x duration)` over that set must
/// fit inside the term. If it does not, the instance is **provably** unplaceable
/// — no ordering, no backtracking, no metaheuristic.
///
/// No per-entity load metric can detect this: every individual can be lightly
/// loaded while the sets still pairwise intersect.
///
/// Greedy gives a *lower* bound on the maximum clique, so this can understate
/// infeasibility but never invent it — a reported value above 1.0 is a genuine
/// certificate, and a value below 1.0 is not a proof of feasibility.
pub fn person_clique(problem: &Problem) -> (usize, u64) {
    // Sessions realizing each Offering: placements plus locked Sessions.
    let mut sessions = vec![0u64; problem.offerings.len()];
    for p in problem.placement_ids() {
        sessions[problem.placement(p).offering.get()] += 1;
    }
    for f in &problem.fixed {
        if let Some(o) = f.offering {
            sessions[o.get()] += 1;
        }
    }

    // Biggest attendee sets first: they are the ones that conflict widely, and
    // a greedy clique grown from them is the tightest bound for the effort.
    // Capped so this stays O(cap x clique x intersect) rather than O(n^2).
    const CANDIDATE_CAP: usize = 400;
    let mut order: Vec<usize> = (0..problem.offerings.len()).collect();
    order.sort_by_key(|&i| {
        (
            std::cmp::Reverse(problem.offerings[i].attendees.len()),
            i, // deterministic tie-break
        )
    });
    order.truncate(CANDIDATE_CAP);

    let shares = |a: usize, b: usize| {
        let (x, y) = (&problem.offerings[a], &problem.offerings[b]);
        // `Problem::build` leaves attendee lists sorted and deduplicated.
        let (small, large) = if x.attendees.len() <= y.attendees.len() { (x, y) } else { (y, x) };
        small
            .attendees
            .iter()
            .any(|p| large.attendees.binary_search(p).is_ok())
    };

    let mut clique: Vec<usize> = Vec::new();
    for &i in &order {
        if problem.offerings[i].attendees.is_empty() {
            continue;
        }
        if clique.iter().all(|&j| shares(i, j)) {
            clique.push(i);
        }
    }

    let blocks = clique
        .iter()
        .map(|&i| sessions[i] * problem.offerings[i].duration_blocks as u64)
        .sum();
    (clique.len(), blocks)
}
