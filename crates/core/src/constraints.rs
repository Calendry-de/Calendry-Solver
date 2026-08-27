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

/// Which catalogue type a violation belongs to.
///
/// A type rather than the eight `&'static str` constants it replaces. The
/// constants were exported, but the service filtered on a raw literal
/// (`v.constraint_type == "ExactFrequency"`) — so renaming a constant's *value*
/// silently disconnected that filter with no compile error, and adding a
/// fifteenth catalogue type gave downstream consumers no signal that they needed
/// to handle it. Both are now compile-time facts.
///
/// [`ViolationType::as_str`] preserves the exact wire strings, so this is not a
/// schema change.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum ViolationType {
    RoomDoubleBooking,
    LecturerDoubleBooking,
    GroupDoubleBooking,
    PersonDoubleBooking,
    ExactFrequency,
    LecturerVeto,
    OnlineOnsiteSameDay,
    MaxOnlineShare,
}

impl ViolationType {
    /// The wire name, unchanged from the constants this replaced.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RoomDoubleBooking => "RoomDoubleBooking",
            Self::LecturerDoubleBooking => "LecturerDoubleBooking",
            Self::GroupDoubleBooking => "GroupDoubleBooking",
            Self::PersonDoubleBooking => "PersonDoubleBooking",
            Self::ExactFrequency => "ExactFrequency",
            Self::LecturerVeto => "LecturerVeto",
            Self::OnlineOnsiteSameDay => "OnlineOnsiteSameDay",
            Self::MaxOnlineShare => "MaxOnlineShare",
        }
    }
}

impl std::fmt::Display for ViolationType {
    fn fmt(&self, w: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        w.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub constraint_id: String,
    pub constraint_type: ViolationType,
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
                    constraint_type: ViolationType::LecturerVeto,
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

/// The two Group-scoped aggregate types, evaluated by replaying the whole
/// solution into a fresh counter set.
///
/// `OnlineOnsiteSameDay` is a filter the search can never violate, so anything
/// reported here came from the caller's immovable input — which the "warn and
/// allow" manual-edit UX can legitimately produce. `MaxOnlineShare` lives on the
/// objective and CAN survive into a returned solution.
pub fn aggregates(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    if problem.constraints.online_onsite_same_day.is_empty()
        && problem.constraints.max_online_share.is_empty()
    {
        return;
    }
    let state = SearchState::replay(problem, solution);

    for instance in &problem.constraints.online_onsite_same_day {
        for (group, day) in state.aggregates.mixed_days() {
            out.push(Violation {
                constraint_id: instance.id.clone(),
                constraint_type: ViolationType::OnlineOnsiteSameDay,
                session_ids: Vec::new(),
                offering_ids: Vec::new(),
                detail: format!(
                    "group '{}' has both online and on-site sessions on day {day}",
                    problem.groups[group.get()].id
                ),
            });
        }
    }

    for (rule_idx, group, window, online, total) in state.aggregates.violated_cells() {
        let rule = &problem.constraints.max_online_share[rule_idx];
        out.push(Violation {
            constraint_id: rule.id.clone(),
            constraint_type: ViolationType::MaxOnlineShare,
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
                    constraint_type: ViolationType::ExactFrequency,
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
    let mut seen: HashSet<(usize, usize, ViolationType, &str)> = HashSet::new();

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
    seen: &mut HashSet<(usize, usize, ViolationType, &'p str)>,
    out: &mut Vec<Violation>,
) {
    let c = &problem.constraints;
    let f = problem.slots.flags(slot);
    // Formatted only when a violation is actually built. Allocating this string
    // for every pair up front — before any check had run — measured at 25% of
    // `structural`, and the overwhelming majority of pairs report nothing.
    let at = SlotLabel(f);

    let mut report = |instance: &'p ConstraintInstance,
                      ty: ViolationType,
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
    if let (Some(rx), Some(ry)) = (x.room, y.room)
        && rx == ry
    {
        for i in c.room_double_booking.iter().filter(|i| both(i)) {
            report(
                i,
                ViolationType::RoomDoubleBooking,
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
                ViolationType::LecturerDoubleBooking,
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
                ViolationType::GroupDoubleBooking,
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
                ViolationType::PersonDoubleBooking,
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
