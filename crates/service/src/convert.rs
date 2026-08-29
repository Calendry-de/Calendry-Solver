//! The boundary between the wire model and the dense search model.
//!
//! Everything string-keyed, `Option`-wrapped and heap-shaped stops here.
//! Downstream of this module the solver addresses entities by `u32` index.
//!
//! This module is also where input validation lives. It is deliberately strict
//! about *structural* problems (a Session on a day the tenant does not teach, a
//! room id that does not exist) and deliberately permissive about *feasibility*
//! problems (a snapshot that already double-books a room). The app's manual-edit
//! UX is "warn and allow", so infeasible input is expected and must degrade
//! gracefully rather than be rejected.
//!
//! Two things about that used to be aspirational rather than true, and both are
//! now enforced by types:
//!
//! * The strictness claim was **false for four call sites**, which dropped
//!   unknown ids with `filter_map`. An unknown `room_id` in particular became
//!   roomless occupancy — structurally invisible to room double-booking. Every
//!   id resolution now goes through [`Resolver`] and names its policy:
//!   `require` or `optional`, with the reason at the call site.
//! * Errors were `tonic::Status` values built in place, so the code-selection
//!   policy had no single home and core's typed errors were flattened to prose.
//!   This module now returns [`ConvertError`]; the mapping to a transport
//!   response lives in [`crate::error`] and nowhere else.

use std::collections::{HashMap, HashSet};

use calendry_solver_core::aggregates::{
    CompactnessInstance, DayMixInstance, PatternAdherenceInstance, ShareInstance, ShareWindow,
};
use calendry_solver_core::ids::{GroupIdx, OfferingIdx, PersonIdx, RoomIdx, SlotIdx};
use calendry_solver_core::preferences::{Preference, PreferenceInstance};
use calendry_solver_core::problem::{
    CapacityWasteInstance, ConstraintInstance, ConstraintSet, FixedSpec, Immovable,
    MaxConcurrentOnlineInstance, OfferingSpec, PlacementVar, Problem, ProblemSpec, Room,
    SchedulingPattern, ScopeSpec, Unavailability, classify_immovable,
};
use calendry_solver_core::slots::{SlotTable, WeekKind, WeekSpec};
use calendry_solver_core::soft::{SoftInstance, SoftParams};
use calendry_solver_core::solution::MAX_ADDITIONAL_ROOMS;
use calendry_solver_proto::v1 as pb;

use crate::dates;
use crate::error::{ConvertError, Resolver};

pub fn convert(input: &pb::SolverInput, scope: &pb::SolveScope) -> Result<Problem, ConvertError> {
    let lock_policy = check_lock_policy(scope)?;

    let slots = build_grid(input)?;
    let rooms = build_rooms(input);
    let room_index = index_by(&input.rooms, |r| r.id.clone());

    let (groups, group_index) = build_groups(input)?;
    let persons = build_persons(input, &group_index)?;
    let person_index = index_by(&input.persons, |p| p.id.clone());

    let reference = resolve_reference(input, &slots);
    let scope_offerings = resolve_scope(input, scope);

    let offerings = build_offerings(input, &rooms, &group_index, &person_index)?;
    let offering_index = index_by(&input.offerings, |o| o.id.clone());

    let indexes = Indexes {
        rooms: Resolver::new(&room_index),
        groups: Resolver::new(&group_index),
        persons: Resolver::new(&person_index),
        offerings: Resolver::new(&offering_index),
    };

    let (placements, mut fixed) = partition_sessions(
        input,
        &slots,
        &indexes,
        &offerings,
        &scope_offerings,
        reference,
        lock_policy,
    )?;

    fixed.extend(build_external_occupancy(input, &slots, &room_index, &rooms)?);

    let constraints = build_constraints(input)?;

    // Scope membership is carried into `Problem` rather than thrown away.
    //
    // It was resolved here, used twice — to classify immovability and to gate
    // placement emission — and then dropped, which left `exact_frequency`
    // reconstructing it downstream from "does this Offering own a placement
    // variable". That inference is lossy in exactly the direction that matters:
    // deducting already-locked Sessions can drive an in-scope Offering's
    // placement count to zero, so an **over-supplied** Offering was
    // indistinguishable from an out-of-scope one and its mismatch went
    // unreported.
    let in_scope: Vec<OfferingIdx> = offerings
        .iter()
        .enumerate()
        .filter(|(_, o)| scope_offerings.contains(&o.id))
        .map(|(i, _)| OfferingIdx(i as u32))
        .collect();

    // One derivation path, shared with the hand-written fixtures and the
    // benchmark generator: group closures and attendee sets are computed inside
    // `Problem::build` so the three callers cannot drift on closure semantics.
    Ok(Problem::build(ProblemSpec {
        rooms,
        groups,
        persons,
        offerings,
        placements,
        fixed,
        constraints,
        scope: ScopeSpec::Offerings(in_scope),
        movement_weight: lock_policy.movement_weight(),
        ..ProblemSpec::new(slots)
    })?)
}

// ---------------------------------------------------------------------------
// Scope & lock policy
// ---------------------------------------------------------------------------

/// The lock policy this conversion will honor, resolved from the wire enum
/// plus (for v2) its weight. `Copy` because it is threaded through
/// `partition_sessions` and read once per existing Session.
#[derive(Copy, Clone, Debug)]
enum ResolvedLockPolicy {
    /// v1: everything outside scope is hard-locked, via `FixedSpec`.
    Hard,
    /// v2: an out-of-scope Session becomes a movable `PlacementVar` carrying
    /// its `original` slot/room, charged `weight` if the search leaves it.
    MinimizeMovement { weight: f64 },
}

impl ResolvedLockPolicy {
    fn movement_weight(self) -> f64 {
        match self {
            Self::Hard => 0.0,
            Self::MinimizeMovement { weight } => weight,
        }
    }
}

fn check_lock_policy(scope: &pb::SolveScope) -> Result<ResolvedLockPolicy, ConvertError> {
    match pb::LockPolicy::try_from(scope.outside_scope_policy) {
        Ok(pb::LockPolicy::Hard) => Ok(ResolvedLockPolicy::Hard),
        Ok(pb::LockPolicy::MinimizeMovement) => {
            let weight = scope.minimize_movement_weight;
            if weight < 0.0 || weight.is_nan() {
                return Err(ConvertError::NegativeMovementWeight { weight });
            }
            Ok(ResolvedLockPolicy::MinimizeMovement { weight })
        }
        _ => Err(ConvertError::LockPolicyUnset),
    }
}

/// An Offering is in scope if it is named directly, or if any of its Groups is.
fn resolve_scope(input: &pb::SolverInput, scope: &pb::SolveScope) -> HashSet<String> {
    let by_id: HashSet<&String> = scope.offering_ids.iter().collect();
    let by_group: HashSet<&String> = scope.group_ids.iter().collect();

    input
        .offerings
        .iter()
        .filter(|o| by_id.contains(&o.id) || o.group_ids.iter().any(|g| by_group.contains(g)))
        .map(|o| o.id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

fn build_grid(input: &pb::SolverInput) -> Result<SlotTable, ConvertError> {
    let grid = input
        .time_grid
        .as_ref()
        .ok_or(ConvertError::MissingTimeGrid)?;
    let calendar = input
        .calendar
        .as_ref()
        .ok_or(ConvertError::MissingCalendar)?;

    let mut weeks = Vec::with_capacity(calendar.weeks.len());
    for w in &calendar.weeks {
        let kind = match pb::WeekKind::try_from(w.kind) {
            Ok(pb::WeekKind::Exam) => WeekKind::Exam,
            Ok(pb::WeekKind::Break) => WeekKind::Break,
            Ok(pb::WeekKind::Holiday) => WeekKind::Holiday,
            // TEACHING and UNSPECIFIED both mean an ordinary week.
            _ => WeekKind::Teaching,
        };

        // Place each holiday on its weekday within this week, so that holiday
        // awareness resolves against the structured calendar rather than any
        // slicing of the week list.
        let mut holiday_weekdays = Vec::new();
        if !w.start_date.is_empty() {
            for h in &calendar.holidays {
                if let Some(day) = dates::weekday_within_week(&w.start_date, &h.date) {
                    holiday_weekdays.push(day);
                }
            }
        }

        weeks.push(WeekSpec { kind, holiday_weekdays });
    }

    SlotTable::build(grid.blocks_per_day, &grid.active_days, &weeks)
        .map_err(|e| ConvertError::InvalidTimeGrid { reason: e.to_string() })
}

/// The caller-supplied "now", resolved to a comparable slot.
///
/// Uses a lower bound rather than an exact resolve because a caller may
/// legitimately hand us an instant falling on a day the tenant does not teach.
/// `None` means the reference lies past the end of the term, which makes every
/// Session in the snapshot past.
fn resolve_reference(input: &pb::SolverInput, slots: &SlotTable) -> Option<SlotIdx> {
    let r = input.reference_slot.as_ref()?;
    slots.lower_bound(r.week, r.day, r.block)
}

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------

fn index_by<T, F: Fn(&T) -> String>(items: &[T], key: F) -> HashMap<String, u32> {
    items
        .iter()
        .enumerate()
        .map(|(i, item)| (key(item), i as u32))
        .collect()
}

fn build_rooms(input: &pb::SolverInput) -> Vec<Room> {
    input
        .rooms
        .iter()
        .map(|r| Room {
            id: r.id.clone(),
            name: r.name.clone(),
            capacity: r.capacity,
            rank: r.rank,
            is_virtual: r.is_virtual,
            features: r.feature_tags.clone(),
            federation_owned: matches!(r.owner, Some(pb::room::Owner::FederationId(_))),
        })
        .collect()
}

type GroupBuild = (Vec<calendry_solver_core::problem::Group>, HashMap<String, u32>);

fn build_groups(input: &pb::SolverInput) -> Result<GroupBuild, ConvertError> {
    let index = index_by(&input.groups, |g| g.id.clone());

    let mut parent_of = Vec::with_capacity(input.groups.len());
    for g in &input.groups {
        let parent = if g.parent_id.is_empty() {
            None
        } else {
            match index.get(&g.parent_id) {
                Some(&p) => Some(GroupIdx(p)),
                None => {
                    return Err(ConvertError::UnknownGroupParent {
                        group: g.id.clone(),
                        parent: g.parent_id.clone(),
                    });
                }
            }
        };
        parent_of.push(parent);
    }

    // The closure itself is derived in `Problem::build` — never transmitted —
    // because shipping the app's closure table would create a second source of
    // truth the solver has no way to check.

    let groups = input
        .groups
        .iter()
        .zip(&parent_of)
        .map(|(g, &parent)| calendry_solver_core::problem::Group {
            id: g.id.clone(),
            parent,
            name: g.name.clone(),
            size: g.size,
            // Verbatim, exactly like `Person.blackouts`: an empty axis means
            // "every value on that axis" and the grid resolves it in
            // `Problem::build`. The app sends the COMPLEMENT of when the Group
            // is available, so nothing here needs to know about dates.
            blackouts: g
                .blackouts
                .iter()
                .map(|b| Unavailability {
                    days: b.days.clone(),
                    blocks: b.blocks.clone(),
                    weeks: b.weeks.clone(),
                })
                .collect(),
        })
        .collect();

    Ok((groups, index))
}

fn build_persons(
    input: &pb::SolverInput,
    group_index: &HashMap<String, u32>,
) -> Result<Vec<calendry_solver_core::problem::Person>, ConvertError> {
    let groups = Resolver::new(group_index);
    input
        .persons
        .iter()
        .map(|p| {
            Ok(calendry_solver_core::problem::Person {
                id: p.id.clone(),
                role_tags: p.role_tags.clone(),
                // REQUIRED. A silently dropped membership removes the Person from
                // that Group's attendee list, so `PersonDoubleBooking` stops seeing
                // a clash that is really there — the check exists precisely to catch
                // what the Group check structurally cannot.
                groups: groups.require_all(&p.group_ids, GroupIdx, |group| {
                    ConvertError::UnknownGroup { context: format!("person '{}'", p.id), group }
                })?,
                // NOTE THE INVERTED EMPTINESS against `blackouts` below: an
                // absent `Preference` and one with two empty axes both mean "no
                // preference", where an empty axis on an `Unavailability` means
                // "every value on that axis". The two messages are structurally
                // identical and semantically opposite, which is why the wire
                // keeps them separate and why this conversion does too.
                //
                // Narrowing to the grid and clamping the multiplier happen in
                // `PreferenceModel::build`, alongside the rest of the table —
                // not here — so there is one place that decides what a stale
                // stored value means.
                preferred: p.preferred.as_ref().map(|pref| Preference {
                    days: pref.days.clone(),
                    blocks: pref.blocks.clone(),
                    // References `Room.feature_tags`' vocabulary by key, not
                    // by an id — the same tradeoff `Offering.
                    // required_room_features` already accepts. Not narrowed
                    // or validated here: an unknown key simply never matches
                    // any Room's features, the same "stale value is inert"
                    // reading `MinimizeBlockUsage` and a stale day/block value
                    // already get.
                    room_features: pref.preferred_room_features.clone(),
                    weight_multiplier: pref.weight_multiplier,
                }),
                // An empty list on an axis means "every value on that axis", which
                // is preserved verbatim rather than normalised here — the grid is
                // what resolves it, in `Problem::build`.
                blackouts: p
                    .blackouts
                    .iter()
                    .map(|b| Unavailability {
                        days: b.days.clone(),
                        blocks: b.blocks.clone(),
                        weeks: b.weeks.clone(),
                    })
                    .collect(),
            })
        })
        .collect()
}

fn build_offerings(
    input: &pb::SolverInput,
    rooms: &[Room],
    group_index: &HashMap<String, u32>,
    person_index: &HashMap<String, u32>,
) -> Result<Vec<OfferingSpec>, ConvertError> {
    let groups = Resolver::new(group_index);
    let persons = Resolver::new(person_index);
    let mut out = Vec::with_capacity(input.offerings.len());

    for o in &input.offerings {
        // v1 takes lecturers as already assigned. A genuine pool is a materially
        // larger search space and is rejected rather than silently mis-solved;
        // the schema does not need to change when it lands.
        if o.candidate_lecturer_ids.len() as u32 != o.required_lecturer_count {
            return Err(ConvertError::LecturerPoolUnsupported {
                offering: o.id.clone(),
                required: o.required_lecturer_count,
                candidates: o.candidate_lecturer_ids.len(),
            });
        }

        if o.duration_blocks == 0 {
            return Err(ConvertError::ZeroDurationOffering { offering: o.id.clone() });
        }

        let allowed: HashSet<&String> = o.allowed_room_ids.iter().collect();
        let required: HashSet<&String> = o.required_room_features.iter().collect();

        // Every filter EXCEPT capacity, which for a multi-Room Offering is
        // evaluated per-COMBINATION below rather than per-Room.
        let individually_eligible = |i: usize, r: &Room| {
            if !allowed.is_empty() && !allowed.contains(&r.id) {
                return false;
            }
            if r.is_virtual && !o.allow_online {
                return false;
            }
            if !required.iter().all(|f| r.features.contains(f)) {
                return false;
            }
            // Quantity-aware, additive alongside the presence-only check
            // above rather than replacing it: `required_room_features` and
            // `room_feature_requirements` are different wire lists a caller
            // is not required to keep in sync, so both are honored.
            room_feature_requirements_met(&input.rooms[i], r, &o.room_feature_requirements)
        };

        if o.required_room_count > MAX_ROOMS_PER_SESSION {
            return Err(ConvertError::TooManyRoomsRequired {
                offering: o.id.clone(),
                required: o.required_room_count,
                max: MAX_ROOMS_PER_SESSION,
            });
        }

        let (eligible_rooms, eligible_room_combinations) = if o.required_room_count > 1 {
            let pool: Vec<RoomIdx> = rooms
                .iter()
                .enumerate()
                .filter(|(i, r)| individually_eligible(*i, r))
                .map(|(i, _)| RoomIdx(i as u32))
                .collect();
            (
                vec![],
                room_combinations(&pool, rooms, o.required_room_count as usize, o.min_capacity),
            )
        } else {
            let eligible: Vec<RoomIdx> = rooms
                .iter()
                .enumerate()
                .filter(|(i, r)| individually_eligible(*i, r) && r.capacity >= o.min_capacity)
                .map(|(i, _)| RoomIdx(i as u32))
                .collect();
            (eligible, vec![])
        };

        out.push(OfferingSpec {
            id: o.id.clone(),
            kind: o.kind.clone(),
            required_session_count: o.required_session_count,
            duration_blocks: o.duration_blocks,
            // All three REQUIRED. A dropped lecturer still passes the
            // `required_lecturer_count` gate above — it was checked against the
            // *wire* list — so the Offering would silently be solved with fewer
            // lecturers than the caller assigned, and lecturer double-booking
            // would not police the missing one.
            lecturers: persons.require_all(&o.candidate_lecturer_ids, PersonIdx, |person| {
                ConvertError::UnknownPerson {
                    context: format!("offering '{}' lecturers", o.id),
                    person,
                }
            })?,
            groups: groups.require_all(&o.group_ids, GroupIdx, |group| {
                ConvertError::UnknownGroup { context: format!("offering '{}'", o.id), group }
            })?,
            participants: persons.require_all(&o.participant_person_ids, PersonIdx, |person| {
                ConvertError::UnknownPerson {
                    context: format!("offering '{}' participants", o.id),
                    person,
                }
            })?,
            eligible_rooms,
            required_room_count: o.required_room_count,
            eligible_room_combinations,
            min_capacity: o.min_capacity,
            // An unrecognized or absent value maps to `Unspecified` — the same
            // "solve exactly as today" inert reading the wire field's own doc
            // comment promises, not an error: a stale value here is not a
            // structural problem the way an unknown id is.
            scheduling_pattern: match pb::SchedulingPattern::try_from(o.scheduling_pattern) {
                Ok(pb::SchedulingPattern::Distributed) => SchedulingPattern::Distributed,
                Ok(pb::SchedulingPattern::Block) => SchedulingPattern::Block,
                _ => SchedulingPattern::Unspecified,
            },
        });
    }

    Ok(out)
}

/// How many Rooms one Session can occupy at once: 1 primary +
/// `MAX_ADDITIONAL_ROOMS`. A real structural limit, refused rather than
/// truncated — unlike [`MAX_ROOM_COMBINATIONS`], which truncates, this is a
/// caller error the caller can actually fix (ask for fewer Rooms).
const MAX_ROOMS_PER_SESSION: u32 = 1 + MAX_ADDITIONAL_ROOMS as u32;

/// Safety cap on how many Room combinations one multi-Room Offering
/// enumerates. Truncated, not refused — consistent with "warn and allow": an
/// Offering whose true combination count exceeds this solves with a
/// (deterministic, prefix) subset of its valid combinations rather than
/// paying an unbounded conversion cost for a pathological Room pool.
const MAX_ROOM_COMBINATIONS: usize = 2000;

/// Every combination of `k` distinct Rooms from `pool` whose SUMMED capacity
/// meets `min_capacity`, in the shape [`calendry_solver_core::problem::Offering::room_choice`]
/// wants: `(primary, additional)`. Capped at [`MAX_ROOM_COMBINATIONS`].
fn room_combinations(
    pool: &[RoomIdx],
    rooms: &[Room],
    k: usize,
    min_capacity: u32,
) -> Vec<(RoomIdx, [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS])> {
    let mut out = Vec::new();
    let mut current: Vec<RoomIdx> = Vec::with_capacity(k);
    room_combinations_go(pool, rooms, k, min_capacity, 0, &mut current, &mut out);
    out
}

#[allow(clippy::too_many_arguments)]
fn room_combinations_go(
    pool: &[RoomIdx],
    rooms: &[Room],
    k: usize,
    min_capacity: u32,
    start: usize,
    current: &mut Vec<RoomIdx>,
    out: &mut Vec<(RoomIdx, [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS])>,
) {
    if k == 0 || out.len() >= MAX_ROOM_COMBINATIONS {
        return;
    }
    if current.len() == k {
        let total_capacity: u32 = current.iter().map(|r| rooms[r.get()].capacity).sum();
        if total_capacity >= min_capacity {
            let mut additional = [None; MAX_ADDITIONAL_ROOMS];
            for (slot, &r) in additional.iter_mut().zip(&current[1..]) {
                *slot = Some(r);
            }
            out.push((current[0], additional));
        }
        return;
    }
    for i in start..pool.len() {
        if out.len() >= MAX_ROOM_COMBINATIONS {
            return;
        }
        current.push(pool[i]);
        room_combinations_go(pool, rooms, k, min_capacity, i + 1, current, out);
        current.pop();
    }
}

/// Whether every quantity-aware requirement on an Offering is satisfied by
/// `wire_room`.
///
/// Additive alongside `required_room_features`'s presence-only check, not a
/// replacement — see the call site. A requirement with no stated
/// `min_quantity` asks the SAME question `required_room_features` already
/// does (feature presence), through the new list's syntax rather than a new
/// mechanism, per the field's own doc comment in `model.proto`; a stated
/// minimum compares counts against `Room.feature_quantities`, the fix this
/// exists for. `min_quantity: Some(0)` is a vacuous requirement any Room
/// satisfies — not special-cased, since it falls out of the comparison.
fn room_feature_requirements_met(
    wire_room: &pb::Room,
    room: &Room,
    requirements: &[pb::RoomFeatureRequirement],
) -> bool {
    requirements.iter().all(|req| {
        let have = wire_room
            .feature_quantities
            .iter()
            .find(|fq| fq.feature == req.feature)
            .map_or(0, |fq| fq.quantity);
        match req.min_quantity {
            Some(min) => have >= min,
            None => room.features.contains(&req.feature) || have > 0,
        }
    })
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// The index maps `partition_sessions` resolves against.
///
/// Grouped into one struct so the function is not eight positional arguments,
/// four of which were interchangeable `&HashMap<String, u32>` values — a
/// transposition between them would have compiled.
struct Indexes<'a> {
    rooms: Resolver<'a>,
    groups: Resolver<'a>,
    persons: Resolver<'a>,
    offerings: Resolver<'a>,
}

fn partition_sessions(
    input: &pb::SolverInput,
    slots: &SlotTable,
    ix: &Indexes<'_>,
    offerings: &[OfferingSpec],
    scope_offerings: &HashSet<String>,
    reference: Option<SlotIdx>,
    lock_policy: ResolvedLockPolicy,
) -> Result<(Vec<PlacementVar>, Vec<FixedSpec>), ConvertError> {
    let mut fixed = Vec::new();
    // Existing in-scope Sessions, per Offering, so a re-solve preserves Session
    // ids instead of churning them downstream.
    let mut reusable: HashMap<String, Vec<String>> = HashMap::new();
    // Existing out-of-scope Sessions made movable by
    // `LOCK_POLICY_MINIMIZE_MOVEMENT`, one `PlacementVar` per Session, carrying
    // `original` so the search can be charged for leaving it. Kept separate
    // from `reusable`/the outstanding-count loop below: those are about NEW
    // demand for an in-scope Offering, and this is neither — no new Session is
    // needed, and the Offering is not in scope.
    let mut movable_out_of_scope: Vec<PlacementVar> = Vec::new();
    let mut movable_occurrence: HashMap<usize, u32> = HashMap::new();

    for s in &input.existing_sessions {
        let sr = s
            .start_slot
            .as_ref()
            .ok_or_else(|| ConvertError::SessionWithoutStart { session: s.id.clone() })?;

        let start = slots.resolve(sr.week, sr.day, sr.block).ok_or_else(|| {
            ConvertError::SessionOffGrid {
                session: s.id.clone(),
                week: sr.week,
                day: sr.day,
                block: sr.block,
            }
        })?;

        let in_scope = !s.offering_id.is_empty() && scope_offerings.contains(&s.offering_id);

        // The Offering this Session realizes, if any. Ad-hoc Sessions (a
        // `staff_meeting` kind) legitimately realize none, and a Session naming
        // an Offering absent from the snapshot resolves to `None` rather than
        // erroring — the caller's "warn and allow" editing UX can produce that,
        // and it is occupancy either way.
        // OPTIONAL, and the documented reason: an ad-hoc Session (a
        // `staff_meeting` kind) legitimately realizes no Offering, and a Session
        // naming an Offering absent from this snapshot is occupancy either way —
        // the caller's "warn and allow" editing UX produces both.
        let offering = ix.offerings.optional(&s.offering_id, OfferingIdx);

        let reason = classify_immovable(start, reference, s.is_locked, in_scope);

        // The ONLY variant v2 relaxes (ADR-0008), and only when it CAN be
        // relaxed: a `PlacementVar` has no room for its own lecturers/groups —
        // every other placement is governed entirely by its Offering — so an
        // ad-hoc Session (no Offering) has nothing to attach "movable" to and
        // stays hard-locked regardless of policy. The moment a Session becomes
        // movable it is a placement like any other for its Offering, so unlike
        // `FixedSpec` below it does NOT carry its own lecturer/group/attendee
        // snapshot — the search reads those from the Offering's current
        // definition, same as it does for every other placement.
        let movable = matches!(reason, Some(Immovable::OutOfScope))
            && matches!(lock_policy, ResolvedLockPolicy::MinimizeMovement { .. })
            && offering.is_some();

        if movable {
            let offering = offering.expect("checked by `movable` above");
            let room =
                if s.room_id.is_empty() {
                    None
                } else {
                    Some(ix.rooms.require(&s.room_id, RoomIdx, |room| {
                        ConvertError::UnknownRoom { context: format!("session '{}'", s.id), room }
                    })?)
                };
            let occurrence = movable_occurrence.entry(offering.get()).or_insert(0);
            movable_out_of_scope.push(PlacementVar {
                offering,
                occurrence: *occurrence,
                existing_session_id: Some(s.id.clone()),
                original: Some((start, room)),
            });
            *occurrence += 1;
            continue;
        }

        match reason {
            Some(reason) => fixed.push(FixedSpec {
                session_id: s.id.clone(),
                offering,
                kind: s.kind.clone(),
                // REQUIRED unless genuinely absent.
                //
                // An unknown `room_id` used to resolve to `None`, which made the
                // Session **roomless occupancy** — it still blocked its
                // lecturers and groups, but room double-booking structurally
                // could not see it, so the solver would happily place another
                // Session in a room a locked Session was already using. An empty
                // `room_id` is different and stays permitted: an online-only or
                // not-yet-roomed Session is a real state.
                room: if s.room_id.is_empty() {
                    None
                } else {
                    Some(ix.rooms.require(&s.room_id, RoomIdx, |room| {
                        ConvertError::UnknownRoom { context: format!("session '{}'", s.id), room }
                    })?)
                },
                // `room_ids` is the AUTHORITATIVE full set, `room_id` included,
                // for a Session occupying more than one Room — empty means
                // unchanged single-Room behavior, `room` alone is already the
                // complete answer. Extra entries beyond `MAX_ADDITIONAL_ROOMS`
                // are truncated, not refused, same "warn and allow" reasoning
                // as everywhere else in this module.
                additional_rooms: {
                    let mut additional = [None; MAX_ADDITIONAL_ROOMS];
                    let extras = s.room_ids.iter().filter(|id| id.as_str() != s.room_id);
                    for (slot, id) in additional.iter_mut().zip(extras) {
                        *slot = Some(ix.rooms.require(id, RoomIdx, |room| {
                            ConvertError::UnknownRoom {
                                context: format!("session '{}'", s.id),
                                room,
                            }
                        })?);
                    }
                    additional
                },
                start,
                duration_blocks: s.duration_blocks.max(1),
                // REQUIRED. A dropped lecturer, group or attendee silently
                // narrows what this immovable Session blocks, which is the same
                // failure as the room case one axis over.
                lecturers: ix
                    .persons
                    .require_all(&s.lecturer_ids, PersonIdx, |person| {
                        ConvertError::UnknownPerson {
                            context: format!("session '{}' lecturers", s.id),
                            person,
                        }
                    })?,
                groups: ix.groups.require_all(&s.group_ids, GroupIdx, |group| {
                    ConvertError::UnknownGroup { context: format!("session '{}'", s.id), group }
                })?,
                persons: ix.persons.require_all(&s.person_ids, PersonIdx, |person| {
                    ConvertError::UnknownPerson {
                        context: format!("session '{}' attendees", s.id),
                        person,
                    }
                })?,
                reason,
            }),
            None => reusable
                .entry(s.offering_id.clone())
                .or_default()
                .push(s.id.clone()),
        }
    }

    // Deterministic id reuse: sort so that the mapping does not depend on the
    // caller's ordering of existing_sessions.
    for ids in reusable.values_mut() {
        ids.sort();
    }

    // Immovable Sessions already realizing each in-scope Offering. These are
    // Sessions that exist and are not going to move — locked, in the past, or
    // out of scope — so the run must place the REMAINDER, not the full count.
    // A movable out-of-scope Session is deliberately NOT counted here: it left
    // `fixed` for `movable_out_of_scope` above, and its Offering is not in
    // scope, so no in-scope `outstanding` count below can see it either way.
    let mut already_realized = vec![0u32; offerings.len()];
    for f in &fixed {
        if let Some(o) = f.offering {
            already_realized[o.get()] += 1;
        }
    }

    let mut placements = movable_out_of_scope;
    for (i, o) in offerings.iter().enumerate() {
        if !scope_offerings.contains(&o.id) {
            continue;
        }
        // `saturating_sub`, not `-`: the app's editing UX is "warn and allow",
        // so a caller can legitimately send more Sessions than the Offering
        // claims to need. Wrapping a u32 here would ask the solver to place four
        // billion Sessions. Over-supply instead yields zero placements, and
        // `ExactFrequency` reports the mismatch as the violation it is.
        let outstanding = o.required_session_count.saturating_sub(already_realized[i]);

        let reuse = reusable.remove(&o.id).unwrap_or_default();
        for occurrence in 0..outstanding {
            placements.push(PlacementVar {
                offering: OfferingIdx(i as u32),
                occurrence,
                existing_session_id: reuse.get(occurrence as usize).cloned(),
                original: None,
            });
        }
    }

    Ok((placements, fixed))
}

fn build_external_occupancy(
    input: &pb::SolverInput,
    slots: &SlotTable,
    room_index: &HashMap<String, u32>,
    rooms: &[Room],
) -> Result<Vec<FixedSpec>, ConvertError> {
    let mut out = Vec::new();

    for e in &input.external_occupancy {
        let Some(&room) = room_index.get(&e.room_id) else {
            return Err(ConvertError::UnknownRoom {
                context: "external_occupancy".to_string(),
                room: e.room_id.clone(),
            });
        };

        // Occupancy from another tenant only makes sense against a shared room.
        if !rooms[room as usize].federation_owned {
            return Err(ConvertError::ExternalOccupancyOnPrivateRoom { room: e.room_id.clone() });
        }

        let sr = e
            .start_slot
            .as_ref()
            .ok_or(ConvertError::ExternalOccupancyWithoutStart)?;
        let Some(start) = slots.resolve(sr.week, sr.day, sr.block) else {
            // An external booking outside this tenant's grid cannot collide with
            // anything the solver places, so it is dropped rather than rejected.
            continue;
        };

        out.push(FixedSpec {
            // Another tenant's use of a Federation-shared Room. It realizes no
            // Offering in *this* snapshot, so it is occupancy and nothing more.
            offering: None,
            session_id: if e.source_ref.is_empty() {
                format!("external:{}", e.room_id)
            } else {
                format!("external:{}", e.source_ref)
            },
            kind: String::new(),
            room: Some(RoomIdx(room)),
            additional_rooms: [None; MAX_ADDITIONAL_ROOMS],
            start,
            duration_blocks: e.duration_blocks.max(1),
            lecturers: Vec::new(),
            groups: Vec::new(),
            persons: Vec::new(),
            reason: Immovable::External,
        });
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Constraints
// ---------------------------------------------------------------------------

fn build_constraints(input: &pb::SolverInput) -> Result<ConstraintSet, ConvertError> {
    use pb::constraint_config::Params;

    let mut set = ConstraintSet::default();

    for c in &input.constraints {
        if !c.enabled {
            continue;
        }

        // `applies_to_kinds` empty means "all kinds". A type may be configured
        // more than once with different kind scopes, which is why each is a list.
        let instance = ConstraintInstance { id: c.id.clone(), kinds: c.applies_to_kinds.clone() };

        match &c.params {
            Some(Params::RoomDoubleBooking(_)) => set.room_double_booking.push(instance),
            Some(Params::LecturerDoubleBooking(_)) => set.lecturer_double_booking.push(instance),
            Some(Params::GroupDoubleBooking(_)) => set.group_double_booking.push(instance),
            Some(Params::PersonDoubleBooking(_)) => set.person_double_booking.push(instance),
            Some(Params::ExactFrequency(_)) => set.exact_frequency.push(instance),

            Some(Params::LecturerVeto(_)) => set.lecturer_veto.push(instance),
            // The Group counterpart. Same empty message, same "values live on
            // the entity, this switches enforcement on" split.
            Some(Params::GroupVeto(_)) => set.group_veto.push(instance),
            /*
             * SOFT since the reclassification, so it reads `weight` like every
             * other soft type and lands in its own list rather than in the
             * filter set. A tenant that has not been backfilled sends weight 0,
             * which the solver treats as "count it, do not steer" — the same
             * reading every soft type gives a zero weight, and the reason the
             * app's rollout order puts the backfill before the deploy.
             */
            Some(Params::OnlineOnsiteSameDay(_)) => {
                if c.weight < 0.0 || c.weight.is_nan() {
                    // The same fault class as every other soft type's weight, so
                    // the same variant: one name per fault, not one per site.
                    return Err(ConvertError::NegativeSoftWeight {
                        constraint: c.id.clone(),
                        weight: c.weight,
                    });
                }
                set.online_onsite_same_day.push(DayMixInstance {
                    id: c.id.clone(),
                    kinds: c.applies_to_kinds.clone(),
                    weight: c.weight,
                });
            }
            Some(Params::MaxOnlineShare(p)) => {
                if !(0.0..=1.0).contains(&p.max_ratio) || p.max_ratio.is_nan() {
                    return Err(ConvertError::ShareRatioOutOfRange {
                        constraint: c.id.clone(),
                        ratio: p.max_ratio,
                    });
                }
                let window = match pb::ShareWindow::try_from(p.window) {
                    Ok(pb::ShareWindow::PerTerm) => ShareWindow::PerTerm,
                    Ok(pb::ShareWindow::PerWeek) => ShareWindow::PerWeek,
                    _ => {
                        return Err(ConvertError::ShareWindowUnset { constraint: c.id.clone() });
                    }
                };
                set.max_online_share.push(ShareInstance {
                    id: c.id.clone(),
                    kinds: c.applies_to_kinds.clone(),
                    max_ratio: p.max_ratio,
                    window,
                });
            }

            // The soft types. Weight is meaningful only here — hard types ignore
            // it, because hard-vs-soft is a property of the TYPE.
            //
            // The two block variants below are DEPRECATED on the wire and must
            // still be accepted: deprecation removes them from what senders
            // should emit, not from what a peer on the old schema may already be
            // sending. Refusing them would turn a schema upgrade on one side
            // into a rejected run on the other.
            #[allow(deprecated)]
            Some(Params::MinimizeFirstBlock(_)) => {
                set.soft
                    .push(soft_instance(c, SoftParams::MinimizeFirstBlock)?);
            }
            #[allow(deprecated)]
            Some(Params::MinimizeLastBlock(_)) => {
                set.soft
                    .push(soft_instance(c, SoftParams::MinimizeLastBlock)?);
            }
            Some(Params::MinimizeBlockUsage(p)) => {
                // Deliberately NOT validated against blocks_per_day. A grid may
                // shrink under a constraint that named a higher index, and this
                // repo's rule is that the solver tolerates input the app's
                // warn-and-allow UX can produce; a stale index is inert in
                // `applies`, not a rejected run.
                //
                // A rule that selects nothing at all IS rejected, because it can
                // only be a configuration mistake: it carries a weight, costs
                // scoring time, and can never fire.
                if p.blocks.is_empty() && !p.first && !p.last {
                    return Err(ConvertError::BlockUsageSelectsNothing {
                        constraint: c.id.clone(),
                    });
                }

                set.soft.push(soft_instance(
                    c,
                    SoftParams::MinimizeBlockUsage {
                        blocks: p.blocks.clone(),
                        first: p.first,
                        last: p.last,
                    },
                )?);
            }
            Some(Params::MinimizeDayUsage(p)) => {
                for d in &p.days {
                    if !(1..=7).contains(d) {
                        return Err(ConvertError::NotAnIsoWeekday {
                            constraint: c.id.clone(),
                            day: *d,
                        });
                    }
                }
                set.soft
                    .push(soft_instance(c, SoftParams::MinimizeDayUsage { days: p.days.clone() })?);
            }
            Some(Params::MinimizeRoomRank(p)) => set.soft.push(soft_instance(
                c,
                SoftParams::MinimizeRoomRank { rank_threshold: p.rank_threshold, invert: p.invert },
            )?),
            Some(Params::MinimizeExamWeek(p)) => {
                set.soft
                    .push(soft_instance(c, SoftParams::MinimizeExamWeek { invert: p.invert })?);
            }
            Some(Params::MinimizeOnline(_)) => {
                set.soft.push(soft_instance(c, SoftParams::MinimizeOnline)?);
            }
            /*
             * SOFT, and its own list rather than `set.soft`, because a
             * preference cost is keyed by PLACEMENT — it depends on who leads
             * the Session — and `SoftModel` is a `(profile, slot, room)` table.
             * Same reason `OnlineOnsiteSameDay` has its own list.
             *
             * `roles` is REFUSED when non-empty rather than approximated. Empty
             * means "lecturers only", which is the decided scope: a Session's
             * attendee set includes every member of every attached Group's
             * descendant closure, so counting attendees would let a 200-student
             * cohort's aggregate preference outweigh the person teaching. The
             * field exists so that scope stays decidable without another schema
             * bump; until it is decided, widening the counted set silently is
             * exactly the failure the offering-scope skip exists to prevent.
             */
            Some(Params::PersonPreferenceFit(p)) => {
                if !p.roles.is_empty() {
                    return Err(ConvertError::PreferenceRolesUnsupported {
                        constraint: c.id.clone(),
                        roles: p.roles.clone(),
                    });
                }
                if c.weight < 0.0 || c.weight.is_nan() {
                    // The same fault class as every other soft weight, so the
                    // same variant. A negative weight here would invert the type
                    // into "penalize honouring a preference".
                    return Err(ConvertError::NegativeSoftWeight {
                        constraint: c.id.clone(),
                        weight: c.weight,
                    });
                }
                set.person_preference_fit.push(PreferenceInstance {
                    id: c.id.clone(),
                    kinds: c.applies_to_kinds.clone(),
                    weight: c.weight,
                });
            }
            /*
             * SOFT, day-granularity, its own list for the same reason
             * `online_onsite_same_day` has one — see
             * `crate::problem::ConstraintSet::compactness`'s own doc.
             *
             * `scope` empty means both axes, matching the field's own proto
             * comment; `try_from` maps an unrecognized enum value to
             * `Unspecified`, which counts toward NEITHER axis rather than
             * erroring — the same "ignore, do not reject" reading
             * `PersonPreferenceFit.roles` being empty already gets, since a
             * caller sending a stale value here is not asking for a rule that
             * cannot exist, only for one that does nothing extra.
             */
            Some(Params::Compactness(p)) => {
                if c.weight < 0.0 || c.weight.is_nan() {
                    return Err(ConvertError::NegativeSoftWeight {
                        constraint: c.id.clone(),
                        weight: c.weight,
                    });
                }
                let (mut group, mut person) = (false, false);
                for &s in &p.scope {
                    match pb::CompactnessScope::try_from(s) {
                        Ok(pb::CompactnessScope::Group) => group = true,
                        Ok(pb::CompactnessScope::Person) => person = true,
                        _ => {}
                    }
                }
                if p.scope.is_empty() {
                    group = true;
                    person = true;
                }
                set.compactness.push(CompactnessInstance {
                    id: c.id.clone(),
                    kinds: c.applies_to_kinds.clone(),
                    weight: c.weight,
                    group,
                    person,
                });
            }
            Some(Params::LecturerConsistency(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "LecturerConsistency",
                });
            }
            /*
             * SOFT, aggregate over an Offering's placed Sessions — see
             * `crate::problem::ConstraintSet::distributed_pattern_adherence`'s
             * own doc. Empty message: which Offerings this instance actually
             * prices comes from `Offering.scheduling_pattern`, read at
             * `Problem::build` time, not from anything on this message.
             */
            Some(Params::DistributedPatternAdherence(_)) => {
                if c.weight < 0.0 || c.weight.is_nan() {
                    return Err(ConvertError::NegativeSoftWeight {
                        constraint: c.id.clone(),
                        weight: c.weight,
                    });
                }
                set.distributed_pattern_adherence
                    .push(PatternAdherenceInstance {
                        id: c.id.clone(),
                        kinds: c.applies_to_kinds.clone(),
                        weight: c.weight,
                    });
            }
            Some(Params::BlockPatternAdherence(_)) => {
                if c.weight < 0.0 || c.weight.is_nan() {
                    return Err(ConvertError::NegativeSoftWeight {
                        constraint: c.id.clone(),
                        weight: c.weight,
                    });
                }
                set.block_pattern_adherence.push(PatternAdherenceInstance {
                    id: c.id.clone(),
                    kinds: c.applies_to_kinds.clone(),
                    weight: c.weight,
                });
            }

            // -- P2 batch, staged together for one version bump --
            //
            // Each refused as UNIMPLEMENTED until its own tracking issue lands
            // an evaluator, same discipline `LecturerConsistency` above uses:
            // a tenant configuring one of these gets a clear refusal, never a
            // silently inert setting.
            // Built — see `crate::problem::Problem::capacity_waste_cost`.
            Some(Params::MinimizeCapacityWaste(p)) => {
                if c.weight < 0.0 || c.weight.is_nan() {
                    return Err(ConvertError::NegativeSoftWeight {
                        constraint: c.id.clone(),
                        weight: c.weight,
                    });
                }
                set.minimize_capacity_waste.push(CapacityWasteInstance {
                    id: c.id.clone(),
                    kinds: c.applies_to_kinds.clone(),
                    weight: c.weight,
                    waste_ratio_threshold: p.waste_ratio_threshold,
                });
            }
            Some(Params::MinimizeLocationChange(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "MinimizeLocationChange",
                });
            }
            Some(Params::MaxConsecutiveBlocks(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "MaxConsecutiveBlocks",
                });
            }
            Some(Params::MaxWeeklyTeachingLoad(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "MaxWeeklyTeachingLoad",
                });
            }
            Some(Params::ExamSpacingSameDay(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "ExamSpacingSameDay",
                });
            }
            Some(Params::ExamSpacingWindow(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "ExamSpacingWindow",
                });
            }
            Some(Params::ProtectedBlock(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "ProtectedBlock",
                });
            }
            Some(Params::RoomConsistency(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "RoomConsistency",
                });
            }
            Some(Params::MinimizeRoomChurn(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "MinimizeRoomChurn",
                });
            }
            Some(Params::MaxDailySpan(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "MaxDailySpan",
                });
            }
            Some(Params::MinimizeWeekdayImbalance(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "MinimizeWeekdayImbalance",
                });
            }
            Some(Params::RoomTurnaroundBuffer(_)) => {
                return Err(ConvertError::ConstraintTypeUnimplemented {
                    constraint: c.id.clone(),
                    constraint_type: "RoomTurnaroundBuffer",
                });
            }
            // Built — see `crate::problem::Problem::max_concurrent_online`.
            // `kinds` is intentionally not read: every online Session counts
            // toward this cap, whatever kind it realizes.
            Some(Params::MaxConcurrentOnlineSessions(p)) => {
                set.max_concurrent_online_sessions
                    .push(MaxConcurrentOnlineInstance {
                        id: c.id.clone(),
                        max_concurrent: p.max_concurrent,
                    });
            }
            // Built — see `crate::problem::ConstraintSet::group_size_fits_room`.
            // No parameters: the values (`Group.size`, `Room.capacity`)
            // already exist; this only switches the cross-check on.
            Some(Params::GroupSizeFitsRoom(_)) => {
                set.group_size_fits_room.push(instance);
            }

            None => {
                return Err(ConvertError::ConstraintWithoutParams { constraint: c.id.clone() });
            }
        }
    }

    // Every catalogue type the schema defines is now evaluated, including
    // `PersonPreferenceFit`. What is still refused is one PARAMETER of it — a
    // non-empty `roles` — rather than the type.
    //
    // The property that matters is unchanged: a new type added to the schema
    // fails to COMPILE against this match rather than being silently ignored.
    // That is how `PersonPreferenceFit` announced itself when the 0.7.0 pin
    // landed, and it is why there is no `_ =>` arm.
    Ok(set)
}

/// Build a soft instance, rejecting a negative weight.
///
/// Every soft type declares "minimize". A negative weight would silently invert
/// it into a maximize the type never declared — so it is refused rather than
/// quietly honoured. Zero is fine and means "report the count, do not steer".
fn soft_instance(
    c: &pb::ConstraintConfig,
    params: SoftParams,
) -> Result<SoftInstance, ConvertError> {
    if c.weight < 0.0 || c.weight.is_nan() {
        return Err(ConvertError::NegativeSoftWeight {
            constraint: c.id.clone(),
            weight: c.weight,
        });
    }
    Ok(SoftInstance {
        id: c.id.clone(),
        kinds: c.applies_to_kinds.clone(),
        weight: c.weight,
        params,
    })
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

pub fn build_output(
    problem: &Problem,
    outcome: &calendry_solver_core::SolveOutcome,
    elapsed_millis: u64,
) -> pb::SolverOutput {
    let mut sessions = Vec::new();

    for p in problem.placement_ids() {
        let Some(placement) = outcome.solution.get(p) else {
            continue;
        };
        let var = problem.placement(p);
        let offering = problem.offering_of(p);
        let f = problem.slots.flags(placement.start);

        sessions.push(pb::PlacedSession {
            session_id: var.existing_session_id.clone().unwrap_or_default(),
            // Same value a hard violation's `session_ids` carries for this
            // placement (`Problem::placement_label`), so a violation naming a
            // Session this run invented — whose `session_id` above is
            // deliberately empty — still resolves to this entry.
            placement_ref: problem.placement_label(p),
            offering_id: offering.id.clone(),
            start_slot: Some(pb::SlotRef { week: f.week, day: f.iso_weekday, block: f.block }),
            duration_blocks: offering.duration_blocks,
            room_id: problem.rooms[placement.room.get()].id.clone(),
            lecturer_ids: offering
                .lecturers
                .iter()
                .map(|&l| problem.persons[l.get()].id.clone())
                .collect(),
            group_ids: offering
                .own_groups
                .iter()
                .map(|&g| problem.groups[g.get()].id.clone())
                .collect(),
            person_ids: offering
                .participants
                .iter()
                .map(|&x| problem.persons[x.get()].id.clone())
                .collect(),
            // `room_id` above stays the primary Room, meaningful on its own
            // for an ordinary single-Room Session. This is populated with the
            // FULL set, `room_id` included, only when this Session actually
            // occupies more than one Room — an ordinary Session gets an empty
            // list here rather than a redundant one-element echo of `room_id`.
            room_ids: if placement.additional_rooms.iter().any(Option::is_some) {
                placement
                    .all_rooms()
                    .map(|r| problem.rooms[r.get()].id.clone())
                    .collect()
            } else {
                Vec::new()
            },
        });
    }

    let hard_violations = outcome
        .hard_violations
        .iter()
        .map(|v| pb::ConstraintViolation {
            constraint_id: v.constraint_id.clone(),
            constraint_type: v.constraint_type.to_string(),
            session_ids: v.session_ids.clone(),
            offering_ids: v.offering_ids.clone(),
            detail: v.detail.clone(),
        })
        .collect();

    let components: Vec<pb::ComponentScore> =
        calendry_solver_core::search::soft_breakdown(problem, &outcome.solution)
            .into_iter()
            .map(|c| pb::ComponentScore {
                constraint_id: c.constraint_id,
                constraint_type: c.constraint_type.to_string(),
                raw_count: c.raw_count,
                weighted: c.weighted,
            })
            .collect();

    pb::SolverOutput {
        sessions,
        hard_violations,
        objective: Some(pb::ObjectiveBreakdown {
            total: outcome.objective.total(problem.hard_penalty),
            components,
        }),
        stats: Some(pb::SolveStats {
            moves_evaluated: outcome.moves_evaluated,
            moves_accepted: outcome.moves_accepted,
            elapsed_millis,
            termination_reason: outcome.termination_reason.to_string(),
        }),
        // DRAFT field, not wired: see its comment in model.proto. The search
        // produces one result; there is nothing to put here yet.
        candidates: Vec::new(),
    }
}
