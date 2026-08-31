//! Constraint evaluators.
//!
//! One typed, compiled function per constraint type. There is no interpreter
//! and no expression language: tenant-supplied logic never executes. Adding a
//! type is a code change here, by design.
//!
//! This module is the **authoritative** check. The occupancy index inside
//! [`crate::solution::SearchState`] is what the constructive heuristic uses to
//! *avoid* creating violations, and it is deliberately conservative about kind
//! scoping; the pairwise rules below are exact. See
//! `docs/adr/0014-structural-stays-independent-of-occupancy.md` for why this
//! duplication is kept.

use std::collections::{HashMap, HashSet};

use crate::ids::{GroupIdx, OfferingIdx, PersonIdx, RoomIdx, SlotIdx};
use crate::problem::{ConstraintInstance, Problem};
use crate::solution::{MAX_ADDITIONAL_ROOMS, MAX_LECTURERS, SearchState, Solution};

/// Which catalogue type a report belongs to.
///
/// A type rather than the eight `&'static str` constants it replaces. The
/// constants were exported, but the service filtered on a raw literal
/// (`v.constraint_type == "ExactFrequency"`) — so renaming a constant's *value*
/// silently disconnected that filter with no compile error, and adding a
/// catalogue type gave downstream consumers no signal that they needed to handle
/// it. Both are now compile-time facts.
///
/// Named for the **constraint**, not for the violation, because a type here can
/// now be reported through either of two channels.
/// `OnlineOnsiteSameDay` is the case that forced the distinction: it used to be a
/// hard filter and is now priced on the objective, so it is carried in
/// [`crate::soft::SoftComponent`] rather than in a [`Violation`]. It is still the
/// same catalogue type with the same wire name.
///
/// [`ConstraintType::as_str`] preserves the exact wire strings, so this is not a
/// schema change.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum ConstraintType {
    RoomDoubleBooking,
    LecturerDoubleBooking,
    GroupDoubleBooking,
    PersonDoubleBooking,
    ExactFrequency,
    LecturerVeto,
    GroupVeto,
    OnlineOnsiteSameDay,
    MaxOnlineShare,
    PersonPreferenceFit,
    GroupSizeFitsRoom,
    MaxConcurrentOnlineSessions,
    DifferentTimeRelation,
    MaxDays,
    MaxConsecutiveDays,
}

impl ConstraintType {
    /// The wire name, unchanged from the constants this replaced.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RoomDoubleBooking => "RoomDoubleBooking",
            Self::LecturerDoubleBooking => "LecturerDoubleBooking",
            Self::GroupDoubleBooking => "GroupDoubleBooking",
            Self::PersonDoubleBooking => "PersonDoubleBooking",
            Self::ExactFrequency => "ExactFrequency",
            Self::LecturerVeto => "LecturerVeto",
            Self::GroupVeto => "GroupVeto",
            Self::OnlineOnsiteSameDay => "OnlineOnsiteSameDay",
            Self::MaxOnlineShare => "MaxOnlineShare",
            Self::PersonPreferenceFit => "PersonPreferenceFit",
            Self::GroupSizeFitsRoom => "GroupSizeFitsRoom",
            Self::MaxConcurrentOnlineSessions => "MaxConcurrentOnlineSessions",
            Self::DifferentTimeRelation => "DifferentTimeRelation",
            Self::MaxDays => "MaxDays",
            Self::MaxConsecutiveDays => "MaxConsecutiveDays",
        }
    }
}

impl std::fmt::Display for ConstraintType {
    fn fmt(&self, w: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        w.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub constraint_id: String,
    pub constraint_type: ConstraintType,
    pub session_ids: Vec<String>,
    pub offering_ids: Vec<String>,
    pub detail: String,
}

/// `week W day D block B`, rendered on demand.
///
/// `Display` rather than a `String` so the allocation happens only inside the
/// `format!` that builds a real violation message.
struct SlotLabel<'a>(&'a crate::slots::SlotFlags);

impl std::fmt::Display for SlotLabel<'_> {
    fn fmt(&self, w: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(w, "week {} day {} block {}", self.0.week, self.0.iso_weekday, self.0.block)
    }
}

/// A Session occupying slots, whether immovable or placed by this run.
struct View<'a> {
    label: String,
    kind: &'a str,
    room: Option<RoomIdx>,
    additional_rooms: [Option<RoomIdx>; MAX_ADDITIONAL_ROOMS],
    /// A fixed assignment's lecturers, or empty for a pool Offering's placed
    /// Session — see [`Self::all_lecturers`], which is what every check
    /// actually reads.
    lecturers: &'a [PersonIdx],
    /// A pool Offering's placed Session's CHOSEN lecturers — see
    /// [`crate::solution::Placement::lecturers`]. `[None; MAX_LECTURERS]`
    /// for a fixed assignment or an immovable Session, both of which carry
    /// their lecturers in `lecturers` above instead.
    pool_lecturers: [Option<PersonIdx>; MAX_LECTURERS],
    own_groups: &'a [GroupIdx],
    attendees: &'a [PersonIdx],
    /// `None` for an ad-hoc Session realizing no Offering — never a member
    /// of any `DifferentTime` relation, so `check_pair`'s relation check has
    /// nothing to read.
    different_time_relations: &'a [u32],
    span: Vec<SlotIdx>,
}

impl View<'_> {
    /// Every Room this Session occupies, primary first — mirrors
    /// [`crate::solution::Placement::all_rooms`].
    #[inline]
    fn all_rooms(&self) -> impl Iterator<Item = RoomIdx> + '_ {
        self.room
            .into_iter()
            .chain(self.additional_rooms.iter().flatten().copied())
    }

    /// Every lecturer this Session has — mirrors
    /// [`crate::solution::Occupant::all_lecturers`], the same union of the
    /// two mutually-exclusive sources.
    #[inline]
    fn all_lecturers(&self) -> impl Iterator<Item = PersonIdx> + '_ {
        self.lecturers
            .iter()
            .copied()
            .chain(self.pool_lecturers.iter().flatten().copied())
    }
}

/// Evaluate every enabled hard constraint over a complete solution.
///
/// Deterministic ordering throughout: constraints in a fixed sequence, slots
/// ascending, sessions in index order. Two runs with the same seed must produce
/// byte-identical violation lists, so this must never iterate a `HashMap`.
///
/// The four phases below are individually public so a caller can attribute this
/// function's cost without the crate carrying a clock. Same reasoning as
/// [`crate::solution::Occupant::enforce`] being public: measurement should drive
/// the real code path rather than a reimplementation that can drift from it.
pub fn evaluate_hard(problem: &Problem, solution: &Solution) -> Vec<Violation> {
    let mut out = Vec::new();
    exact_frequency(problem, solution, &mut out);
    structural(problem, solution, &mut out);
    lecturer_veto(problem, solution, &mut out);
    group_veto(problem, solution, &mut out);
    group_size_fits_room(problem, solution, &mut out);
    max_concurrent_online_sessions(problem, solution, &mut out);
    aggregates(problem, solution, &mut out);
    max_days(problem, solution, &mut out);
    max_consecutive_days(problem, solution, &mut out);
    out
}

/// HARD, filterable. Reports a slot where more than `max_concurrent` online
/// Sessions coexist — pairwise-adjacent to the four structural
/// double-booking types (a COUNT cap rather than exclusivity), so it walks
/// `collect_views` the same way `structural` does: both fixed and placed
/// occupancy count, since a Session's "online-ness" is a static property of
/// its Room, not something only the search's own placements have.
///
/// The search itself cannot create this violation once configured (see
/// `Occupancy::is_free`); this exists for the same reason `structural`'s
/// pairwise checks still run independently — the authoritative check must
/// not simply trust the constructive heuristic, and locked/fixed occupancy
/// the caller supplied is never filtered at all.
pub fn max_concurrent_online_sessions(
    problem: &Problem,
    solution: &Solution,
    out: &mut Vec<Violation>,
) {
    let Some(cap) = problem.max_concurrent_online else {
        return;
    };
    let views = collect_views(problem, solution);

    let mut count = vec![0u32; problem.slots.len()];
    for v in &views {
        if v.room.is_some_and(|r| problem.rooms[r.get()].is_virtual) {
            for &s in &v.span {
                count[s.get()] += 1;
            }
        }
    }

    // Deterministic ordering: slots ascending, same discipline every other
    // phase here follows.
    for (i, &n) in count.iter().enumerate() {
        if n > cap {
            let f = problem.slots.flags(SlotIdx(i as u32));
            out.push(Violation {
                constraint_id: problem
                    .constraints
                    .max_concurrent_online_sessions
                    .first()
                    .map(|c| c.id.clone())
                    .unwrap_or_default(),
                constraint_type: ConstraintType::MaxConcurrentOnlineSessions,
                session_ids: Vec::new(),
                offering_ids: Vec::new(),
                detail: format!("{n} concurrent online Sessions at {}, cap is {cap}", SlotLabel(f)),
            });
        }
    }
}

/// HARD, validation-shaped. Cross-checks a placement's Room capacity
/// (summed across every Room in a multi-Room placement) against the SUMMED
/// `Group.size` of the Offering's own Groups — a safety net against stale or
/// wrong `Offering.min_capacity` input, not a preference.
///
/// Scoped to `own_groups` (the Offering's DIRECT Groups), not the downward
/// closure: an Offering assigned to exactly one Group is the unambiguous
/// case this exists for. Whether a cohort-level Offering should additionally
/// sum its descendant classes' sizes — and whether a non-leaf Group's own
/// `size` already includes them — is an app-side data question, not
/// something the solver can infer; see the tracking issue.
///
/// Evaluated over placed Sessions only, same convention as `lecturer_veto`:
/// immovable occupancy is reported by the caller's own data.
pub fn group_size_fits_room(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    if problem.constraints.group_size_fits_room.is_empty() {
        return;
    }
    for instance in &problem.constraints.group_size_fits_room {
        for p in problem.placement_ids() {
            let Some(pl) = solution.get(p) else { continue };
            let o = problem.offering_of(p);
            if !instance.covers(&o.kind) {
                continue;
            }
            // `capacity == 0` means UNBOUNDED, not "fits nobody" (issue #62)
            // — a Room with nothing recorded can never be reported over
            // capacity, and this is the one other place besides eligibility
            // that reads `Room.capacity` as a real seat count rather than as
            // a bare presence check.
            let unbounded = pl.all_rooms().any(|r| problem.rooms[r.get()].capacity == 0);
            let capacity: u32 = pl
                .all_rooms()
                .map(|r| problem.rooms[r.get()].capacity)
                .sum();
            let attending: u32 = o
                .own_groups
                .iter()
                .map(|g| problem.groups[g.get()].size)
                .sum();
            if !unbounded && attending > capacity {
                out.push(Violation {
                    constraint_id: instance.id.clone(),
                    constraint_type: ConstraintType::GroupSizeFitsRoom,
                    session_ids: vec![problem.placement_label(p)],
                    offering_ids: vec![o.id.clone()],
                    detail: format!(
                        "'{}' seats {attending} (own Groups) in a Room seating only {capacity}",
                        problem.placement_label(p)
                    ),
                });
            }
        }
    }
}

/// HARD, unary. A lecturer is never assigned during their own blackout.
///
/// Evaluated over placed Sessions only: immovable occupancy is reported by the
/// caller's own data, and re-reporting a locked Session the solver cannot move
/// would be noise. Blackout VALUES live on `Person.blackouts`; the constraint
/// instance only switches enforcement on.
pub fn lecturer_veto(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    if problem.constraints.lecturer_veto.is_empty() {
        return;
    }
    for instance in &problem.constraints.lecturer_veto {
        for p in problem.placement_ids() {
            let Some(pl) = solution.get(p) else { continue };
            let o = problem.offering_of(p);
            if !instance.covers(&o.kind) {
                continue;
            }
            let Some(span) = problem.slots.span(pl.start, o.duration_blocks) else {
                continue;
            };
            for &s in &span {
                if !o.veto_slots.contains(s.get()) {
                    continue;
                }
                let f = problem.slots.flags(s);
                let who = o
                    .lecturers
                    .iter()
                    .find(|l| {
                        problem.persons[l.get()]
                            .blackouts
                            .iter()
                            .any(|b| b.matches(f))
                    })
                    .map(|l| problem.persons[l.get()].id.clone())
                    .unwrap_or_default();
                out.push(Violation {
                    constraint_id: instance.id.clone(),
                    constraint_type: ConstraintType::LecturerVeto,
                    session_ids: vec![problem.placement_label(p)],
                    offering_ids: vec![o.id.clone()],
                    detail: format!(
                        "lecturer '{who}' is unavailable at week {} day {} block {}",
                        f.week, f.iso_weekday, f.block
                    ),
                });
                break;
            }
        }
    }
}

/// HARD, unary. A Session is never placed during a blackout of a Group
/// attending it.
///
/// `lecturer_veto` above, one entity across, and deliberately a separate
/// function rather than a parameter of it: the two are separately enableable, a
/// violation has to name which entity was away, and merging them would make
/// "the cohort is on placement" indistinguishable from "the lecturer is on
/// leave" in the report a timetabler reads.
///
/// The blackout of a Group binds that Group and its DESCENDANTS, so the mask
/// this reads was built by walking UP from the Session's own Groups. Getting
/// that direction backwards is invisible on a flat hierarchy — see
/// [`crate::groups::GroupClosure::expand_ancestry`].
pub fn group_veto(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    if problem.constraints.group_veto.is_empty() {
        return;
    }
    for instance in &problem.constraints.group_veto {
        for p in problem.placement_ids() {
            let Some(pl) = solution.get(p) else { continue };
            let o = problem.offering_of(p);
            if !instance.covers(&o.kind) {
                continue;
            }
            let Some(span) = problem.slots.span(pl.start, o.duration_blocks) else {
                continue;
            };
            for &s in &span {
                if !o.group_veto_slots.contains(s.get()) {
                    continue;
                }
                let f = problem.slots.flags(s);
                // Reported from the ancestry set, not from `own_groups`: the
                // Group that declared the blackout may be an ancestor of the
                // one actually attached, and naming the attached child would
                // send a timetabler to a Group with no window on it.
                let who = problem
                    .closure
                    .expand_ancestry(&o.own_groups)
                    .into_iter()
                    .find(|g| {
                        problem.groups[g.get()]
                            .blackouts
                            .iter()
                            .any(|b| b.matches(f))
                    })
                    .map(|g| problem.groups[g.get()].id.clone())
                    .unwrap_or_default();
                out.push(Violation {
                    constraint_id: instance.id.clone(),
                    constraint_type: ConstraintType::GroupVeto,
                    session_ids: vec![problem.placement_label(p)],
                    offering_ids: vec![o.id.clone()],
                    detail: format!(
                        "group '{who}' is unavailable at week {} day {} block {}",
                        f.week, f.iso_weekday, f.block
                    ),
                });
                break;
            }
        }
    }
}

/// The Group-scoped aggregate types that are still HARD.
///
/// `MaxOnlineShare` lives on the objective and CAN survive into a returned
/// solution, so it is reported from here.
///
/// `OnlineOnsiteSameDay` USED TO BE REPORTED HERE AND DELIBERATELY IS NOT ANY
/// MORE. It was a filter the search could never violate, so a mixed day could
/// only have arrived in the caller's immovable input — which made it a hard
/// violation worth naming. Now that it is soft the search produces mixed days
/// on purpose when the alternative costs more, and listing those as hard
/// violations would report the objective doing its job as a defect. They are
/// carried in the objective breakdown instead, where every other soft type's
/// breaches are, with the count and the weighted cost.
pub fn aggregates(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    if problem.constraints.max_online_share.is_empty() {
        return;
    }
    let state = SearchState::replay(problem, solution);

    for (rule_idx, group, window, online, total) in state.aggregates.violated_cells() {
        let rule = &problem.constraints.max_online_share[rule_idx];
        out.push(Violation {
            constraint_id: rule.id.clone(),
            constraint_type: ConstraintType::MaxOnlineShare,
            session_ids: Vec::new(),
            offering_ids: Vec::new(),
            detail: format!(
                "group '{}' has {online} of {total} sessions online in window {window}, \
                 above the {:.0}% cap (allowance {})",
                problem.groups[group.get()].id,
                rule.max_ratio * 100.0,
                rule.allowance(total)
            ),
        });
    }
}

/// HARD, priced at `hard_penalty` rather than a construction filter
/// (ADR-0025) — same stance as `MaxOnlineShare`, and reported the same way:
/// a run can succeed while still reporting a violated day cap.
pub fn max_days(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    if problem.constraints.max_days.is_empty() {
        return;
    }
    let state = SearchState::replay(problem, solution);
    for (is_person, entity, week, observed) in state.aggregates.max_days_violated_cells() {
        let Some(rule) = problem
            .constraints
            .max_days
            .iter()
            .find(|r| if is_person { r.person } else { r.group })
        else {
            continue;
        };
        let name = if is_person {
            &problem.persons[entity as usize].id
        } else {
            &problem.groups[entity as usize].id
        };
        out.push(Violation {
            constraint_id: rule.id.clone(),
            constraint_type: ConstraintType::MaxDays,
            session_ids: Vec::new(),
            offering_ids: Vec::new(),
            detail: format!(
                "{} '{name}' uses {observed} distinct day(s) in week {week}, above the cap of \
                 {}",
                if is_person { "person" } else { "group" },
                rule.max_days
            ),
        });
    }
}

/// The `MaxConsecutiveDays` counterpart of `max_days`.
pub fn max_consecutive_days(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    if problem.constraints.max_consecutive_days.is_empty() {
        return;
    }
    let state = SearchState::replay(problem, solution);
    for (is_person, entity, week, observed) in
        state.aggregates.max_consecutive_days_violated_cells()
    {
        let Some(rule) = problem
            .constraints
            .max_consecutive_days
            .iter()
            .find(|r| if is_person { r.person } else { r.group })
        else {
            continue;
        };
        let name = if is_person {
            &problem.persons[entity as usize].id
        } else {
            &problem.groups[entity as usize].id
        };
        out.push(Violation {
            constraint_id: rule.id.clone(),
            constraint_type: ConstraintType::MaxConsecutiveDays,
            session_ids: Vec::new(),
            offering_ids: Vec::new(),
            detail: format!(
                "{} '{name}' has a run of {observed} consecutive day(s) in week {week}, above \
                 the cap of {}",
                if is_person { "person" } else { "group" },
                rule.max_consecutive_days
            ),
        });
    }
}

// ---------------------------------------------------------------------------
// ExactFrequency
// ---------------------------------------------------------------------------

/// HARD. Each in-scope Offering must be realized by exactly
/// `required_session_count` placed Sessions.
pub fn exact_frequency(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    if problem.constraints.exact_frequency.is_empty() {
        return;
    }

    let mut placed = vec![0u32; problem.offerings.len()];
    for p in problem.placement_ids() {
        if solution.get(p).is_some() {
            placed[problem.placement(p).offering.get()] += 1;
        }
    }

    // Immovable Sessions realize their Offering just as placed ones do. A
    // locked or already-past Session is still a Session that happened, so it
    // counts toward the required frequency — otherwise every Offering carrying
    // a lock would report a shortfall it does not have, and the ordinary
    // mid-term re-solve could never satisfy this constraint at all.
    for o in problem.offering_ids() {
        placed[o.get()] += problem.immovable_count(o);
    }

    for instance in &problem.constraints.exact_frequency {
        for (i, offering) in problem.offerings.iter().enumerate() {
            // Real scope membership, carried on `Problem` from the caller's
            // request. This used to ask whether the Offering owned any placement
            // variable, which is the same question only while nothing can drive
            // an in-scope Offering's placement count to zero. Deducting locked
            // Sessions can, so an **over-supplied** Offering — more locks than it
            // requires — looked exactly like an out-of-scope one and its
            // mismatch went unreported.
            if !problem.in_scope(OfferingIdx(i as u32)) || !instance.covers(&offering.kind) {
                continue;
            }
            let (want, got) = (offering.required_session_count, placed[i]);
            if got != want {
                out.push(Violation {
                    constraint_id: instance.id.clone(),
                    constraint_type: ConstraintType::ExactFrequency,
                    session_ids: Vec::new(),
                    offering_ids: vec![offering.id.clone()],
                    detail: format!(
                        "offering '{}' requires {want} session(s), {got} placed",
                        offering.id
                    ),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The four structural (double-booking) types
// ---------------------------------------------------------------------------

pub fn structural(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    let views = collect_views(problem, solution);
    if views.is_empty() {
        return;
    }

    // slot -> occupying view indices, ascending.
    let mut by_slot: Vec<Vec<usize>> = vec![Vec::new(); problem.slots.len()];
    for (i, v) in views.iter().enumerate() {
        for s in &v.span {
            by_slot[s.get()].push(i);
        }
    }

    // A pair overlapping several blocks must be reported once, not once per
    // block.
    let mut seen: HashSet<(usize, usize, ConstraintType, &str)> = HashSet::new();

    // Scratch for the person axis, allocated once and reused per slot.
    // `clear()` keeps each bucket's capacity, so after the first few slots this
    // stops allocating entirely.
    let check_persons = !problem.constraints.person_double_booking.is_empty();
    let mut by_person: Vec<Vec<usize>> =
        if check_persons { vec![Vec::new(); problem.persons.len()] } else { Vec::new() };
    let mut touched: Vec<usize> = Vec::new();
    let mut person_clash: HashMap<(usize, usize), usize> = HashMap::new();

    for (slot, occupants) in by_slot.iter().enumerate() {
        if occupants.len() < 2 {
            continue;
        }
        let slot = SlotIdx(slot as u32);

        // Person double-booking, bucketed rather than pair-scanned.
        //
        // Asking "do these two Sessions share an attendee" for every pair costs
        // `pairs x attendee-list scan`, which measured at 72% of this function.
        // Inverting it — map each attendee to the Sessions holding them, then
        // look for an attendee held twice — costs the sum of the attendee list
        // lengths instead. At university scale that is ~3.7k operations per slot
        // against ~600k.
        //
        // This stays entirely independent of `Occupancy`: it reads the same
        // `View` attendee lists the pairwise version read, so it remains the
        // authoritative check rather than trusting the heuristic's index.
        person_clash.clear();
        if check_persons {
            for &vi in occupants {
                for p in views[vi].attendees {
                    let bucket = &mut by_person[p.get()];
                    if bucket.is_empty() {
                        touched.push(p.get());
                    }
                    bucket.push(vi);
                }
            }
            for &p in &touched {
                let bucket = &by_person[p];
                if bucket.len() < 2 {
                    continue;
                }
                // `occupants` is ascending, so each bucket is too, and (i, j)
                // with i < j is already the canonical pair order.
                for (i, &a) in bucket.iter().enumerate() {
                    for &b in &bucket[i + 1..] {
                        // The pairwise version reported the FIRST shared
                        // attendee of `x`, and attendee lists are sorted, so
                        // that is the lowest shared index. Keep the minimum here
                        // to reproduce the same message.
                        person_clash
                            .entry((a, b))
                            .and_modify(|e| {
                                if p < *e {
                                    *e = p;
                                }
                            })
                            .or_insert(p);
                    }
                }
            }
            for &p in &touched {
                by_person[p].clear();
            }
            touched.clear();
        }

        // Almost every slot has no person clash at all, and a hash lookup per
        // pair is not free at ~1.6M pairs. Hoisting the emptiness test out of
        // the pair loop keeps the common case to a single branch.
        let any_clash = !person_clash.is_empty();

        for (ai, &a) in occupants.iter().enumerate() {
            for &b in &occupants[ai + 1..] {
                let shared = if any_clash { person_clash.get(&(a, b)).copied() } else { None };
                check_pair(problem, &views[a], &views[b], a, b, slot, shared, &mut seen, out);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_pair<'p>(
    problem: &'p Problem,
    x: &View<'_>,
    y: &View<'_>,
    xi: usize,
    yi: usize,
    slot: SlotIdx,
    // The lowest attendee index shared by this pair, precomputed in
    // `structural` by bucketing. `None` = they share nobody.
    shared_attendee: Option<usize>,
    seen: &mut HashSet<(usize, usize, ConstraintType, &'p str)>,
    out: &mut Vec<Violation>,
) {
    let c = &problem.constraints;
    let f = problem.slots.flags(slot);
    // Formatted only when a violation is actually built. Allocating this string
    // for every pair up front — before any check had run — measured at 25% of
    // `structural`, and the overwhelming majority of pairs report nothing.
    let at = SlotLabel(f);

    let mut report = |instance: &'p ConstraintInstance,
                      ty: ConstraintType,
                      detail: String,
                      out: &mut Vec<Violation>| {
        if !seen.insert((xi, yi, ty, instance.id.as_str())) {
            return;
        }
        out.push(Violation {
            constraint_id: instance.id.clone(),
            constraint_type: ty,
            session_ids: vec![x.label.clone(), y.label.clone()],
            offering_ids: Vec::new(),
            detail,
        });
    };

    // A pair is only constrained when a single configured instance covers BOTH
    // sessions' kinds. A constraint scoped to `lecture` must not police a clash
    // between a lecture and a tenant-defined `staff_meeting`.
    let both = |i: &ConstraintInstance| i.covers(x.kind) && i.covers(y.kind);

    // 1. Room double-booking.
    //
    // Only EXCLUSIVE rooms clash. A virtual room hosts unlimited concurrent
    // Sessions, and the exemption is keyed on the same `Room::is_exclusive`
    // predicate `Occupancy::exclusive_room` uses to decide whether to claim the
    // slot bit at all — so the search cannot refuse a placement this then
    // declines to report, or the reverse.
    if let Some(r) = x.all_rooms().find(|&rx| y.all_rooms().any(|ry| ry == rx))
        && problem.rooms[r.get()].is_exclusive()
    {
        for i in c.room_double_booking.iter().filter(|i| both(i)) {
            report(
                i,
                ConstraintType::RoomDoubleBooking,
                format!(
                    "room '{}' hosts '{}' and '{}' at {at}",
                    problem.rooms[r.get()].id,
                    x.label,
                    y.label
                ),
                out,
            );
        }
    }

    // 2. Lecturer double-booking.
    if let Some(p) = x
        .all_lecturers()
        .find(|p| y.all_lecturers().any(|yp| yp == *p))
    {
        for i in c.lecturer_double_booking.iter().filter(|i| both(i)) {
            report(
                i,
                ConstraintType::LecturerDoubleBooking,
                format!(
                    "lecturer '{}' leads '{}' and '{}' at {at}",
                    problem.persons[p.get()].id,
                    x.label,
                    y.label
                ),
                out,
            );
        }
    }

    // 3. Group double-booking, with nested propagation in both directions.
    //
    // Expressed directly as the pairwise rule — same root-to-leaf path — rather
    // than by intersecting expanded closures, which would wrongly flag siblings
    // that merely share an ancestor.
    let clash = x.own_groups.iter().find_map(|a| {
        y.own_groups
            .iter()
            .find(|b| problem.closure.conflicts(*a, **b))
            .map(|b| (*a, *b))
    });
    if let Some((a, b)) = clash {
        for i in c.group_double_booking.iter().filter(|i| both(i)) {
            let rel = if a == b {
                format!("group '{}'", problem.groups[a.get()].id)
            } else {
                format!(
                    "nested groups '{}' and '{}'",
                    problem.groups[a.get()].id,
                    problem.groups[b.get()].id
                )
            };
            report(
                i,
                ConstraintType::GroupDoubleBooking,
                format!("{rel} attend '{}' and '{}' at {at}", x.label, y.label),
                out,
            );
        }
    }

    // 4. Person double-booking.
    //
    // Catches what the group check structurally cannot: a Person who belongs to
    // two Groups unrelated in the nesting tree, both scheduled at once.
    if let Some(p) = shared_attendee {
        for i in c.person_double_booking.iter().filter(|i| both(i)) {
            report(
                i,
                ConstraintType::PersonDoubleBooking,
                format!(
                    "person '{}' attends '{}' and '{}' at {at}",
                    problem.persons[p].id, x.label, y.label
                ),
                out,
            );
        }
    }

    // 5. `DifferentTime` Offering relations.
    //
    // Not kind-scoped — a relation names specific Offerings, so `both()`
    // (which reads `applies_to_kinds`) does not apply here. Reported once per
    // relation the pair shares, keyed by the relation's own id rather than a
    // `ConstraintInstance`'s, since a relation carries no `kinds` to scope by.
    for &r in x.different_time_relations {
        if !y.different_time_relations.contains(&r) {
            continue;
        }
        let id = problem.different_time_relation_ids[r as usize].as_str();
        if !seen.insert((xi, yi, ConstraintType::DifferentTimeRelation, id)) {
            continue;
        }
        out.push(Violation {
            constraint_id: id.to_string(),
            constraint_type: ConstraintType::DifferentTimeRelation,
            session_ids: vec![x.label.clone(), y.label.clone()],
            offering_ids: Vec::new(),
            detail: format!(
                "relation '{id}' (DifferentTime) is violated by '{}' and '{}' at {at}",
                x.label, y.label
            ),
        });
    }
}

fn collect_views<'a>(problem: &'a Problem, solution: &Solution) -> Vec<View<'a>> {
    let mut views = Vec::with_capacity(problem.fixed.len() + solution.len());

    for f in &problem.fixed {
        let Some(span) = problem.slots.span(f.start, f.duration_blocks) else {
            continue;
        };
        views.push(View {
            label: f.session_id.clone(),
            kind: &f.kind,
            room: f.room,
            additional_rooms: f.additional_rooms,
            lecturers: &f.lecturers,
            pool_lecturers: [None; MAX_LECTURERS],
            own_groups: &f.own_groups,
            attendees: &f.attendees,
            different_time_relations: &f.different_time_relations,
            span,
        });
    }

    for p in problem.placement_ids() {
        let Some(pl) = solution.get(p) else { continue };
        let o = problem.offering_of(p);
        let Some(span) = problem.slots.span(pl.start, o.duration_blocks) else {
            continue;
        };
        views.push(View {
            label: problem.placement_label(p),
            kind: &o.kind,
            room: Some(pl.room),
            additional_rooms: pl.additional_rooms,
            lecturers: &o.lecturers,
            pool_lecturers: pl.lecturers,
            own_groups: &o.own_groups,
            attendees: &o.attendees,
            different_time_relations: &o.different_time_relations,
            span,
        });
    }

    views
}
