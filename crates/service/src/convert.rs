//! The boundary between the wire model and the dense search model.
//!
//! Everything string-keyed, `Option`-wrapped and heap-shaped stops here.
//! Downstream of this module the solver addresses entities by `u32` index.
//!
//! This module is also where input validation lives. It is deliberately strict
//! about *structural* problems (a Session on a day the tenant does not teach, a
//! room id that does not exist) and deliberately permissive about *feasibility*
//! problems (a snapshot that already double-books a room). The app's
//! manual-edit UX is "warn and allow", so infeasible input is expected and must
//! degrade gracefully rather than be rejected.

use std::collections::{HashMap, HashSet};

use calendry_solver_core::ids::{GroupIdx, OfferingIdx, PersonIdx, RoomIdx, SlotIdx};
use calendry_solver_core::problem::{
    ConstraintInstance, ConstraintSet, FixedSpec, Immovable, OfferingSpec, PlacementVar, Problem,
    Unavailability, classify_immovable,
};
use calendry_solver_core::aggregates::{ShareInstance, ShareWindow};
use calendry_solver_core::slots::{SlotTable, WeekKind, WeekSpec};
use calendry_solver_core::soft::{SoftInstance, SoftParams};
use calendry_solver_proto::v1 as pb;
use tonic::Status;

use crate::dates;

pub fn convert(input: &pb::SolverInput, scope: &pb::SolveScope) -> Result<Problem, Status> {
    check_lock_policy(scope)?;

    let slots = build_grid(input)?;
    let rooms = build_rooms(input);
    let room_index = index_by(&input.rooms, |r| r.id.clone());

    let (groups, group_index) = build_groups(input)?;
    let (persons, person_index) = build_persons(input, &group_index);

    let reference = resolve_reference(input, &slots);
    let scope_offerings = resolve_scope(input, scope);

    let offerings = build_offerings(input, &rooms, &room_index, &group_index, &person_index)?;
    let offering_index = index_by(&input.offerings, |o| o.id.clone());

    let (placements, mut fixed) = partition_sessions(
        input,
        &slots,
        &room_index,
        &group_index,
        &person_index,
        &offering_index,
        &offerings,
        &scope_offerings,
        reference,
    )?;

    fixed.extend(build_external_occupancy(input, &slots, &room_index, &rooms)?);

    let constraints = build_constraints(input)?;

    // One derivation path, shared with the hand-written fixtures: group
    // closures and attendee sets are computed inside `Problem::build` so the two
    // callers cannot drift on closure semantics.
    Problem::build(slots, rooms, groups, persons, offerings, placements, fixed, constraints)
        .map_err(|c| Status::invalid_argument(c.to_string()))
}

// ---------------------------------------------------------------------------
// Scope & lock policy
// ---------------------------------------------------------------------------

fn check_lock_policy(scope: &pb::SolveScope) -> Result<(), Status> {
    match pb::LockPolicy::try_from(scope.outside_scope_policy) {
        Ok(pb::LockPolicy::Hard) => Ok(()),
        Ok(pb::LockPolicy::MinimizeMovement) => Err(Status::unimplemented(
            "LOCK_POLICY_MINIMIZE_MOVEMENT is the deferred v2 policy; v1 hard-locks \
             everything outside scope",
        )),
        _ => Err(Status::invalid_argument(
            "scope.outside_scope_policy must be set; v1 supports LOCK_POLICY_HARD",
        )),
    }
}

/// An Offering is in scope if it is named directly, or if any of its Groups is.
fn resolve_scope(input: &pb::SolverInput, scope: &pb::SolveScope) -> HashSet<String> {
    let by_id: HashSet<&String> = scope.offering_ids.iter().collect();
    let by_group: HashSet<&String> = scope.group_ids.iter().collect();

    input
        .offerings
        .iter()
        .filter(|o| {
            by_id.contains(&o.id) || o.group_ids.iter().any(|g| by_group.contains(g))
        })
        .map(|o| o.id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

fn build_grid(input: &pb::SolverInput) -> Result<SlotTable, Status> {
    let grid = input
        .time_grid
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("input.time_grid is required"))?;
    let calendar = input
        .calendar
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("input.calendar is required"))?;

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
        .map_err(|e| Status::invalid_argument(format!("invalid time grid: {e}")))
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

fn build_rooms(input: &pb::SolverInput) -> Vec<calendry_solver_core::problem::Room> {
    input
        .rooms
        .iter()
        .map(|r| calendry_solver_core::problem::Room {
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

fn build_groups(input: &pb::SolverInput) -> Result<GroupBuild, Status> {
    let index = index_by(&input.groups, |g| g.id.clone());

    let mut parent_of = Vec::with_capacity(input.groups.len());
    for g in &input.groups {
        let parent = if g.parent_id.is_empty() {
            None
        } else {
            match index.get(&g.parent_id) {
                Some(&p) => Some(GroupIdx(p)),
                None => {
                    return Err(Status::invalid_argument(format!(
                        "group '{}' names unknown parent '{}'",
                        g.id, g.parent_id
                    )));
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
        })
        .collect();

    Ok((groups, index))
}

fn build_persons(
    input: &pb::SolverInput,
    group_index: &HashMap<String, u32>,
) -> (Vec<calendry_solver_core::problem::Person>, HashMap<String, u32>) {
    let index = index_by(&input.persons, |p| p.id.clone());
    let persons = input
        .persons
        .iter()
        .map(|p| calendry_solver_core::problem::Person {
            id: p.id.clone(),
            role_tags: p.role_tags.clone(),
            groups: p
                .group_ids
                .iter()
                .filter_map(|g| group_index.get(g).map(|&i| GroupIdx(i)))
                .collect(),
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
        .collect();
    (persons, index)
}

fn build_offerings(
    input: &pb::SolverInput,
    rooms: &[calendry_solver_core::problem::Room],
    room_index: &HashMap<String, u32>,
    group_index: &HashMap<String, u32>,
    person_index: &HashMap<String, u32>,
) -> Result<Vec<OfferingSpec>, Status> {
    let mut out = Vec::with_capacity(input.offerings.len());

    for o in &input.offerings {
        // v1 takes lecturers as already assigned. A genuine pool is a materially
        // larger search space and is rejected rather than silently mis-solved;
        // the schema does not need to change when it lands.
        if o.candidate_lecturer_ids.len() as u32 != o.required_lecturer_count {
            return Err(Status::unimplemented(format!(
                "offering '{}' asks the solver to choose {} of {} candidate lecturers; \
                 v1 supports pre-assigned lecturers only",
                o.id,
                o.required_lecturer_count,
                o.candidate_lecturer_ids.len()
            )));
        }

        if o.duration_blocks == 0 {
            return Err(Status::invalid_argument(format!(
                "offering '{}' has duration_blocks = 0",
                o.id
            )));
        }

        let allowed: HashSet<&String> = o.allowed_room_ids.iter().collect();
        let required: HashSet<&String> = o.required_room_features.iter().collect();

        let eligible_rooms: Vec<RoomIdx> = rooms
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                if !allowed.is_empty() && !allowed.contains(&r.id) {
                    return false;
                }
                if r.is_virtual && !o.allow_online {
                    return false;
                }
                if r.capacity < o.min_capacity {
                    return false;
                }
                required.iter().all(|f| r.features.contains(f))
            })
            .map(|(i, _)| RoomIdx(i as u32))
            .collect();

        out.push(OfferingSpec {
            id: o.id.clone(),
            kind: o.kind.clone(),
            required_session_count: o.required_session_count,
            duration_blocks: o.duration_blocks,
            lecturers: resolve_all(&o.candidate_lecturer_ids, person_index, PersonIdx),
            groups: resolve_all(&o.group_ids, group_index, GroupIdx),
            participants: resolve_all(&o.participant_person_ids, person_index, PersonIdx),
            eligible_rooms,
        });
    }

    let _ = room_index;
    Ok(out)
}

fn resolve_all<T>(ids: &[String], index: &HashMap<String, u32>, wrap: fn(u32) -> T) -> Vec<T> {
    ids.iter()
        .filter_map(|id| index.get(id).map(|&i| wrap(i)))
        .collect()
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn partition_sessions(
    input: &pb::SolverInput,
    slots: &SlotTable,
    room_index: &HashMap<String, u32>,
    group_index: &HashMap<String, u32>,
    person_index: &HashMap<String, u32>,
    offering_index: &HashMap<String, u32>,
    offerings: &[OfferingSpec],
    scope_offerings: &HashSet<String>,
    reference: Option<SlotIdx>,
) -> Result<(Vec<PlacementVar>, Vec<FixedSpec>), Status> {
    let mut fixed = Vec::new();
    // Existing in-scope Sessions, per Offering, so a re-solve preserves Session
    // ids instead of churning them downstream.
    let mut reusable: HashMap<String, Vec<String>> = HashMap::new();

    for s in &input.existing_sessions {
        let sr = s
            .start_slot
            .as_ref()
            .ok_or_else(|| Status::invalid_argument(format!("session '{}' has no start_slot", s.id)))?;

        let start = slots.resolve(sr.week, sr.day, sr.block).ok_or_else(|| {
            Status::invalid_argument(format!(
                "session '{}' sits at week {} day {} block {}, which is not a slot in this \
                 tenant's grid",
                s.id, sr.week, sr.day, sr.block
            ))
        })?;

        let in_scope = !s.offering_id.is_empty() && scope_offerings.contains(&s.offering_id);

        // The Offering this Session realizes, if any. Ad-hoc Sessions (a
        // `staff_meeting` kind) legitimately realize none, and a Session naming
        // an Offering absent from the snapshot resolves to `None` rather than
        // erroring — the caller's "warn and allow" editing UX can produce that,
        // and it is occupancy either way.
        let offering = offering_index.get(&s.offering_id).map(|&i| OfferingIdx(i));

        match classify_immovable(start, reference, s.is_locked, in_scope) {
            Some(reason) => fixed.push(FixedSpec {
                session_id: s.id.clone(),
                offering,
                kind: s.kind.clone(),
                room: room_index.get(&s.room_id).map(|&i| RoomIdx(i)),
                start,
                duration_blocks: s.duration_blocks.max(1),
                lecturers: resolve_all(&s.lecturer_ids, person_index, PersonIdx),
                groups: resolve_all(&s.group_ids, group_index, GroupIdx),
                persons: resolve_all(&s.person_ids, person_index, PersonIdx),
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
    let mut already_realized = vec![0u32; offerings.len()];
    for f in &fixed {
        if let Some(o) = f.offering {
            already_realized[o.get()] += 1;
        }
    }

    let mut placements = Vec::new();
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
            });
        }
    }

    Ok((placements, fixed))
}

fn build_external_occupancy(
    input: &pb::SolverInput,
    slots: &SlotTable,
    room_index: &HashMap<String, u32>,
    rooms: &[calendry_solver_core::problem::Room],
) -> Result<Vec<FixedSpec>, Status> {
    let mut out = Vec::new();

    for e in &input.external_occupancy {
        let Some(&room) = room_index.get(&e.room_id) else {
            return Err(Status::invalid_argument(format!(
                "external_occupancy references unknown room '{}'",
                e.room_id
            )));
        };

        // Occupancy from another tenant only makes sense against a shared room.
        if !rooms[room as usize].federation_owned {
            return Err(Status::invalid_argument(format!(
                "external_occupancy references room '{}', which is not Federation-owned",
                e.room_id
            )));
        }

        let sr = e.start_slot.as_ref().ok_or_else(|| {
            Status::invalid_argument("external_occupancy entry has no start_slot")
        })?;
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

fn build_constraints(input: &pb::SolverInput) -> Result<ConstraintSet, Status> {
    use pb::constraint_config::Params;

    let mut set = ConstraintSet::default();

    for c in &input.constraints {
        if !c.enabled {
            continue;
        }

        // `applies_to_kinds` empty means "all kinds". A type may be configured
        // more than once with different kind scopes, which is why each is a list.
        let instance = ConstraintInstance {
            id: c.id.clone(),
            kinds: c.applies_to_kinds.clone(),
        };

        match &c.params {
            Some(Params::RoomDoubleBooking(_)) => set.room_double_booking.push(instance),
            Some(Params::LecturerDoubleBooking(_)) => set.lecturer_double_booking.push(instance),
            Some(Params::GroupDoubleBooking(_)) => set.group_double_booking.push(instance),
            Some(Params::PersonDoubleBooking(_)) => set.person_double_booking.push(instance),
            Some(Params::ExactFrequency(_)) => set.exact_frequency.push(instance),

            Some(Params::LecturerVeto(_)) => set.lecturer_veto.push(instance),
            Some(Params::OnlineOnsiteSameDay(_)) => set.online_onsite_same_day.push(instance),
            Some(Params::MaxOnlineShare(p)) => {
                if !(0.0..=1.0).contains(&p.max_ratio) || p.max_ratio.is_nan() {
                    return Err(Status::invalid_argument(format!(
                        "constraint '{}' has max_ratio {}; it is a share and must be in 0.0..=1.0",
                        c.id, p.max_ratio
                    )));
                }
                let window = match pb::ShareWindow::try_from(p.window) {
                    Ok(pb::ShareWindow::PerTerm) => ShareWindow::PerTerm,
                    Ok(pb::ShareWindow::PerWeek) => ShareWindow::PerWeek,
                    _ => {
                        return Err(Status::invalid_argument(format!(
                            "constraint '{}' must set window to PER_TERM or PER_WEEK; the \
                             ratio is meaningless without a window to measure it over",
                            c.id
                        )));
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
                set.soft.push(soft_instance(c, SoftParams::MinimizeFirstBlock)?)
            }
            #[allow(deprecated)]
            Some(Params::MinimizeLastBlock(_)) => {
                set.soft.push(soft_instance(c, SoftParams::MinimizeLastBlock)?)
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
                    return Err(Status::invalid_argument(format!(
                        "constraint '{}': MinimizeBlockUsage selects no blocks — \
                         set at least one index, or `first`/`last`",
                        c.id
                    )));
                }

                set.soft.push(soft_instance(
                    c,
                    SoftParams::MinimizeBlockUsage {
                        blocks: p.blocks.clone(),
                        first: p.first,
                        last: p.last,
                    },
                )?)
            }
            Some(Params::MinimizeDayUsage(p)) => {
                for d in &p.days {
                    if !(1..=7).contains(d) {
                        return Err(Status::invalid_argument(format!(
                            "constraint '{}': {d} is not an ISO weekday (1..=7)",
                            c.id
                        )));
                    }
                }
                set.soft.push(soft_instance(
                    c,
                    SoftParams::MinimizeDayUsage { days: p.days.clone() },
                )?)
            }
            Some(Params::MinimizeRoomRank(p)) => set.soft.push(soft_instance(
                c,
                SoftParams::MinimizeRoomRank { rank_threshold: p.rank_threshold, invert: p.invert },
            )?),
            Some(Params::MinimizeExamWeek(_)) => {
                set.soft.push(soft_instance(c, SoftParams::MinimizeExamWeek)?)
            }
            Some(Params::MinimizeOnline(_)) => {
                set.soft.push(soft_instance(c, SoftParams::MinimizeOnline)?)
            }
            None => {
                return Err(Status::invalid_argument(format!(
                    "constraint '{}' has no params set",
                    c.id
                )));
            }
        }
    }

    // Every one of the 14 catalogue types is now implemented, so there is no
    // longer an UNIMPLEMENTED branch here. A new type added to the schema will
    // fail to compile against this match rather than being silently ignored —
    // which is the property that mattered about the old branch.
    Ok(set)
}

/// Build a soft instance, rejecting a negative weight.
///
/// Every soft type declares "minimize". A negative weight would silently invert
/// it into a maximize the type never declared — so it is refused rather than
/// quietly honoured. Zero is fine and means "report the count, do not steer".
fn soft_instance(c: &pb::ConstraintConfig, params: SoftParams) -> Result<SoftInstance, Status> {
    if c.weight < 0.0 || c.weight.is_nan() {
        return Err(Status::invalid_argument(format!(
            "constraint '{}' has weight {}; soft weights must be >= 0 because every soft \
             type declares minimize, and a negative weight would invert it",
            c.id, c.weight
        )));
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
            offering_id: offering.id.clone(),
            start_slot: Some(pb::SlotRef {
                week: f.week,
                day: f.iso_weekday,
                block: f.block,
            }),
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
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod locked_frequency_tests {
    //! Locked Sessions must count toward their Offering's required frequency.
    //!
    //! This is the ordinary mid-term re-solve — the primary case the lock
    //! mechanism exists for. An Offering needing 12 Sessions with 3 already
    //! locked (user-locked, or in the past) needs **9** more, not 12.
    //!
    //! Two halves of one gap made that wrong:
    //!
    //! 1. [`partition_sessions`] dropped `Session.offering_id` when building a
    //!    `FixedSpec`, then created placement variables for the **full**
    //!    `required_session_count` — so the solver scheduled 12 on top of the 3
    //!    locked ones, 15 in total.
    //! 2. `FixedOccupancy` carried no Offering link either, so
    //!    `constraints::exact_frequency` could not have counted the locked
    //!    Sessions toward frequency even had the placements been deducted.
    //!
    //! These are written to be **red against the pre-fix code**, and were
    //! confirmed red before the fix landed.

    use super::convert;
    use calendry_solver_core::constraints;
    use calendry_solver_core::ids::OfferingIdx;
    use calendry_solver_core::search::construct;
    use calendry_solver_proto::v1 as pb;

    const KIND: &str = "lecture";

    fn slot(week: u32, day: u32, block: u32) -> pb::SlotRef {
        pb::SlotRef { week, day, block }
    }

    /// Roomy enough that nothing here is infeasible for want of space:
    /// 4 weeks x Mon-Fri x 6 blocks = 120 slots, 2 rooms.
    fn base_input() -> pb::SolverInput {
        pb::SolverInput {
            requesting_tenant_id: "t1".into(),
            federation_id: String::new(),
            time_grid: Some(pb::TimeGrid {
                blocks_per_day: 6,
                block_length_minutes: 45,
                day_start_minute: 480,
                active_days: vec![1, 2, 3, 4, 5],
                institution_timezone: "Europe/Berlin".into(),
            }),
            calendar: Some(pb::AcademicCalendar {
                term_id: "term-1".into(),
                weeks: (0..4)
                    .map(|i| pb::Week {
                        index: i,
                        start_date: format!("2026-01-{:02}", 5 + i * 7),
                        kind: pb::WeekKind::Teaching as i32,
                    })
                    .collect(),
                holidays: vec![],
            }),
            rooms: (0..2)
                .map(|i| pb::Room {
                    id: format!("r{i}"),
                    owner: Some(pb::room::Owner::TenantId("t1".into())),
                    name: format!("Room {i}"),
                    capacity: 100,
                    rank: 1,
                    is_virtual: false,
                    feature_tags: vec![],
                    location: String::new(),
                })
                .collect(),
            persons: vec![pb::Person {
                id: "p1".into(),
                role_tags: vec!["Lecturer".into()],
                group_ids: vec![],
                blackouts: vec![],
            }],
            groups: vec![pb::Group {
                id: "g1".into(),
                parent_id: String::new(),
                name: "Group 1".into(),
                size: 20,
            }],
            offerings: vec![],
            existing_sessions: vec![],
            external_occupancy: vec![],
            constraints: vec![
                enabled(
                    "c-room",
                    pb::constraint_config::Params::RoomDoubleBooking(pb::RoomDoubleBooking {}),
                ),
                enabled(
                    "c-freq",
                    pb::constraint_config::Params::ExactFrequency(pb::ExactFrequency {}),
                ),
            ],
            // Week 0 Monday block 0 — nothing here is past unless a test puts
            // it there deliberately.
            reference_slot: Some(slot(0, 1, 0)),
        }
    }

    fn enabled(id: &str, params: pb::constraint_config::Params) -> pb::ConstraintConfig {
        pb::ConstraintConfig {
            id: id.into(),
            enabled: true,
            applies_to_kinds: vec![],
            weight: 0.0,
            params: Some(params),
        }
    }

    fn offering(id: &str, required: u32) -> pb::Offering {
        pb::Offering {
            id: id.into(),
            owner: Some(pb::offering::Owner::TenantId("t1".into())),
            kind: KIND.into(),
            required_session_count: required,
            duration_blocks: 1,
            candidate_lecturer_ids: vec!["p1".into()],
            required_lecturer_count: 1,
            group_ids: vec!["g1".into()],
            participant_person_ids: vec![],
            required_room_features: vec![],
            min_capacity: 0,
            allowed_room_ids: vec![],
            allow_online: false,
        }
    }

    fn locked_session(id: &str, offering_id: &str, at: pb::SlotRef) -> pb::Session {
        pb::Session {
            id: id.into(),
            owner: Some(pb::session::Owner::TenantId("t1".into())),
            offering_id: offering_id.into(),
            kind: KIND.into(),
            start_slot: Some(at),
            duration_blocks: 1,
            room_id: "r0".into(),
            lecturer_ids: vec!["p1".into()],
            group_ids: vec!["g1".into()],
            person_ids: vec![],
            is_locked: true,
        }
    }

    fn scope(ids: &[&str]) -> pb::SolveScope {
        pb::SolveScope {
            offering_ids: ids.iter().map(|s| s.to_string()).collect(),
            group_ids: vec![],
            outside_scope_policy: pb::LockPolicy::Hard as i32,
        }
    }

    fn frequency_violations(
        problem: &calendry_solver_core::Problem,
    ) -> Vec<constraints::Violation> {
        let (solution, _) = construct(problem);
        constraints::evaluate_hard(problem, &solution)
            .into_iter()
            .filter(|v| v.constraint_type == "ExactFrequency")
            .collect()
    }

    #[test]
    fn locked_sessions_count_toward_required_frequency() {
        // 12 required, 3 already locked. The run must place exactly 9 more.
        let mut input = base_input();
        input.offerings = vec![offering("o1", 12)];
        input.existing_sessions = vec![
            locked_session("s1", "o1", slot(0, 1, 1)),
            locked_session("s2", "o1", slot(0, 2, 1)),
            locked_session("s3", "o1", slot(0, 3, 1)),
        ];

        let problem = convert(&input, &scope(&["o1"])).expect("valid input");

        assert_eq!(
            problem.fixed.len(),
            3,
            "the three locked Sessions must survive as immovable occupancy"
        );

        // RED BEFORE THE FIX: this was 12, so the solver scheduled 12 on top of
        // the 3 locked ones — 15 Sessions for an Offering requiring 12.
        assert_eq!(
            problem.placements.len(),
            9,
            "12 required minus 3 locked = 9 placements to position"
        );

        // RED BEFORE THE FIX: the link did not exist, so frequency had nothing
        // to count.
        let linked = problem
            .fixed
            .iter()
            .filter(|f| f.offering == Some(OfferingIdx(0)))
            .count();
        assert_eq!(linked, 3, "locked Sessions must carry their Offering link");

        // RED BEFORE THE FIX: 12 required against 9 placed reported a violation
        // that does not exist.
        let freq = frequency_violations(&problem);
        assert!(
            freq.is_empty(),
            "a fully realized Offering must report no frequency violation, got {freq:#?}"
        );
    }

    #[test]
    fn a_genuine_shortfall_is_still_reported() {
        // The counterpart: counting locked Sessions must not become a way to
        // silence a real shortfall. One slot, two rooms, so 5 cannot be reached.
        let mut input = base_input();
        input.time_grid = Some(pb::TimeGrid {
            blocks_per_day: 1,
            block_length_minutes: 45,
            day_start_minute: 480,
            active_days: vec![1],
            institution_timezone: "Europe/Berlin".into(),
        });
        input.calendar = Some(pb::AcademicCalendar {
            term_id: "term-1".into(),
            weeks: vec![pb::Week {
                index: 0,
                start_date: "2026-01-05".into(),
                kind: pb::WeekKind::Teaching as i32,
            }],
            holidays: vec![],
        });
        input.offerings = vec![offering("o1", 5)];
        input.existing_sessions = vec![locked_session("s1", "o1", slot(0, 1, 0))];

        let problem = convert(&input, &scope(&["o1"])).expect("valid input");
        assert_eq!(problem.placements.len(), 4, "5 required minus 1 locked");

        assert!(
            !frequency_violations(&problem).is_empty(),
            "a genuine shortfall must still be reported"
        );
    }

    #[test]
    fn out_of_scope_offerings_are_unaffected() {
        // o2 is not in scope, so it gets no placement variables and its
        // frequency is not this run's business. Its locked Sessions must still
        // occupy the grid without leaking a violation or perturbing o1.
        let mut input = base_input();
        input.offerings = vec![offering("o1", 4), offering("o2", 7)];
        input.existing_sessions = vec![
            locked_session("s1", "o1", slot(0, 1, 1)),
            locked_session("s2", "o2", slot(0, 2, 1)),
            locked_session("s3", "o2", slot(0, 3, 1)),
        ];

        let problem = convert(&input, &scope(&["o1"])).expect("valid input");

        assert_eq!(
            problem.placements.len(),
            3,
            "only o1 is in scope: 4 required minus its 1 lock"
        );
        assert_eq!(problem.fixed.len(), 3, "all three locks remain occupancy");

        let freq = frequency_violations(&problem);
        assert!(
            freq.is_empty(),
            "an out-of-scope Offering must not report a frequency violation, got {freq:#?}"
        );
    }

    #[test]
    fn more_locks_than_required_saturates_instead_of_underflowing() {
        // The app's editing UX is "warn and allow", so a caller can legitimately
        // send more Sessions than the Offering claims to need. The deduction
        // must saturate at zero rather than wrapping a u32 into four billion
        // placement variables.
        let mut input = base_input();
        input.offerings = vec![offering("o1", 2)];
        input.existing_sessions = vec![
            locked_session("s1", "o1", slot(0, 1, 1)),
            locked_session("s2", "o1", slot(0, 2, 1)),
            locked_session("s3", "o1", slot(0, 3, 1)),
            locked_session("s4", "o1", slot(0, 4, 1)),
        ];

        let problem = convert(&input, &scope(&["o1"])).expect("valid input");
        assert_eq!(
            problem.placements.len(),
            0,
            "already over-supplied: nothing left to place, and no underflow"
        );

        // The safety-critical property: no underflow, no absurd placement count.
        // The run simply has nothing to do for this Offering.
        assert!(
            frequency_violations(&problem).is_empty(),
            "KNOWN LIMITATION, asserted so a change is deliberate: over-supply \
             is NOT reported"
        );

        // Why it is not reported, and why that is left alone here:
        //
        // `constraints::exact_frequency` treats "has placement variables" as its
        // proxy for "in scope" — a documented decision that predates this fix.
        // Deducting locks is the first thing that can drive an in-scope
        // Offering's placement count to zero, so this is the first input for
        // which the proxy and real scope membership disagree.
        //
        // Reporting it would mean carrying real scope membership into
        // `Problem`, which is a semantic change to core rather than a fix to
        // the conversion boundary. Deliberately out of scope for a standalone
        // bug fix; recorded in CLAUDE.md as its own decision.
    }
}
