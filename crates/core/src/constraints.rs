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
use crate::solution::{SearchState, Solution};

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
    lecturers: &'a [PersonIdx],
    own_groups: &'a [GroupIdx],
    attendees: &'a [PersonIdx],
    span: Vec<SlotIdx>,
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
    aggregates(problem, solution, &mut out);
    out
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
    if let (Some(rx), Some(ry)) = (x.room, y.room)
        && rx == ry
        && problem.rooms[rx.get()].is_exclusive()
    {
        for i in c.room_double_booking.iter().filter(|i| both(i)) {
            report(
                i,
                ConstraintType::RoomDoubleBooking,
                format!(
                    "room '{}' hosts '{}' and '{}' at {at}",
                    problem.rooms[rx.get()].id,
                    x.label,
                    y.label
                ),
                out,
            );
        }
    }

    // 2. Lecturer double-booking.
    if let Some(p) = x.lecturers.iter().find(|l| y.lecturers.contains(l)) {
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
            lecturers: &f.lecturers,
            own_groups: &f.own_groups,
            attendees: &f.attendees,
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
            lecturers: &o.lecturers,
            own_groups: &o.own_groups,
            attendees: &o.attendees,
            span,
        });
    }

    views
}
