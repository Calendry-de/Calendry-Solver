//! Constraint evaluators.
//!
//! One typed, compiled function per constraint type. There is no interpreter
//! and no expression language: tenant-supplied logic never executes. Adding a
//! type is a code change here, by design.
//!
//! This module is the **authoritative** check. [`crate::solution::Occupancy`] is
//! an index the constructive heuristic uses to *avoid* creating violations, and
//! it is deliberately conservative about kind scoping; the pairwise rules below
//! are exact.

use std::collections::HashSet;

use crate::ids::{GroupIdx, PersonIdx, RoomIdx, SlotIdx};
use crate::problem::{ConstraintInstance, Problem};
use crate::solution::Solution;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub constraint_id: String,
    pub constraint_type: &'static str,
    pub session_ids: Vec<String>,
    pub offering_ids: Vec<String>,
    pub detail: String,
}

pub const ROOM_DOUBLE_BOOKING: &str = "RoomDoubleBooking";
pub const LECTURER_DOUBLE_BOOKING: &str = "LecturerDoubleBooking";
pub const GROUP_DOUBLE_BOOKING: &str = "GroupDoubleBooking";
pub const PERSON_DOUBLE_BOOKING: &str = "PersonDoubleBooking";
pub const EXACT_FREQUENCY: &str = "ExactFrequency";
pub const LECTURER_VETO: &str = "LecturerVeto";
pub const ONLINE_ONSITE_SAME_DAY: &str = "OnlineOnsiteSameDay";
pub const MAX_ONLINE_SHARE: &str = "MaxOnlineShare";

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
                    constraint_type: LECTURER_VETO,
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
    let state = crate::search::rebuild_state(problem, solution);

    for instance in &problem.constraints.online_onsite_same_day {
        for (group, day) in state.aggregates.mixed_days() {
            out.push(Violation {
                constraint_id: instance.id.clone(),
                constraint_type: ONLINE_ONSITE_SAME_DAY,
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
            constraint_type: MAX_ONLINE_SHARE,
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
    let mut in_scope = vec![false; problem.offerings.len()];
    for p in problem.placement_ids() {
        let o = problem.placement(p).offering.get();
        in_scope[o] = true;
        if solution.get(p).is_some() {
            placed[o] += 1;
        }
    }

    // Immovable Sessions realize their Offering just as placed ones do. A
    // locked or already-past Session is still a Session that happened, so it
    // counts toward the required frequency — otherwise every Offering carrying
    // a lock would report a shortfall it does not have, and the ordinary
    // mid-term re-solve could never satisfy this constraint at all.
    //
    // This does NOT mark the Offering in scope: an Offering whose only presence
    // is immovable has no placement variables, so its frequency is not this
    // run's business and the `in_scope` gate below still skips it.
    for f in &problem.fixed {
        if let Some(o) = f.offering {
            placed[o.get()] += 1;
        }
    }

    for instance in &problem.constraints.exact_frequency {
        for (i, offering) in problem.offerings.iter().enumerate() {
            // An Offering with no placement variables is out of scope for this
            // run; its frequency is not this run's business.
            if !in_scope[i] || !instance.covers(&offering.kind) {
                continue;
            }
            let (want, got) = (offering.required_session_count, placed[i]);
            if got != want {
                out.push(Violation {
                    constraint_id: instance.id.clone(),
                    constraint_type: EXACT_FREQUENCY,
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
    let mut seen: HashSet<(usize, usize, &'static str, &str)> = HashSet::new();

    for (slot, occupants) in by_slot.iter().enumerate() {
        if occupants.len() < 2 {
            continue;
        }
        let slot = SlotIdx(slot as u32);

        for (ai, &a) in occupants.iter().enumerate() {
            for &b in &occupants[ai + 1..] {
                check_pair(problem, &views[a], &views[b], a, b, slot, &mut seen, out);
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
    seen: &mut HashSet<(usize, usize, &'static str, &'p str)>,
    out: &mut Vec<Violation>,
) {
    let c = &problem.constraints;
    let f = problem.slots.flags(slot);
    let at = format!("week {} day {} block {}", f.week, f.iso_weekday, f.block);

    let mut report =
        |instance: &'p ConstraintInstance, ty: &'static str, detail: String, out: &mut Vec<Violation>| {
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
                ROOM_DOUBLE_BOOKING,
                format!(
                    "room '{}' hosts '{}' and '{}' at {at}",
                    problem.rooms[rx.get()].id, x.label, y.label
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
                LECTURER_DOUBLE_BOOKING,
                format!(
                    "lecturer '{}' leads '{}' and '{}' at {at}",
                    problem.persons[p.get()].id, x.label, y.label
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
                GROUP_DOUBLE_BOOKING,
                format!("{rel} attend '{}' and '{}' at {at}", x.label, y.label),
                out,
            );
        }
    }

    // 4. Person double-booking.
    //
    // Catches what the group check structurally cannot: a Person who belongs to
    // two Groups unrelated in the nesting tree, both scheduled at once.
    if let Some(p) = x.attendees.iter().find(|p| y.attendees.binary_search(p).is_ok()) {
        for i in c.person_double_booking.iter().filter(|i| both(i)) {
            report(
                i,
                PERSON_DOUBLE_BOOKING,
                format!(
                    "person '{}' attends '{}' and '{}' at {at}",
                    problem.persons[p.get()].id, x.label, y.label
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
