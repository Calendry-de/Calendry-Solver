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
use crate::problem::{ConstraintInstance, Problem, RelationKind};
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
    SameTimeRelation,
    SameDaysRelation,
    SameStartRelation,
    MeetTogetherRelation,
    PrecedenceRelation,
    LecturerRoomPin,
    /// Carried in [`crate::soft::SoftComponent`] rather than in a
    /// [`Violation`], like `OnlineOnsiteSameDay` and `PersonPreferenceFit`
    /// above: it is priced, not filtered. It is here rather than reached
    /// through `SoftParams::type_name` because ADR-0033 moved it out of the
    /// `(kind-profile, slot, room)` table — the wire string is unchanged.
    MinimizeExamWeek,
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
            Self::SameTimeRelation => "SameTimeRelation",
            Self::SameDaysRelation => "SameDaysRelation",
            Self::SameStartRelation => "SameStartRelation",
            Self::MeetTogetherRelation => "MeetTogetherRelation",
            Self::PrecedenceRelation => "PrecedenceRelation",
            Self::LecturerRoomPin => "LecturerRoomPin",
            Self::MinimizeExamWeek => "MinimizeExamWeek",
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
    /// The same, for `MeetTogether` — see `Offering::meet_together_relations`.
    /// Only membership is needed here; `check_pair`'s combined-capacity
    /// reporting reads `Offering::min_capacity` directly instead of through
    /// a `View`, since it groups by relation and week rather than by pair.
    meet_together_relations: &'a [u32],
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
    lecturer_room_pin(problem, solution, &mut out);
    group_size_fits_room(problem, solution, &mut out);
    max_concurrent_online_sessions(problem, solution, &mut out);
    aggregates(problem, solution, &mut out);
    max_days(problem, solution, &mut out);
    max_consecutive_days(problem, solution, &mut out);
    same_relations(problem, solution, &mut out);
    meet_together_relations(problem, solution, &mut out);
    precedence_relations(problem, solution, &mut out);
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

/// `LecturerRoomPin`: a lecturer pinned to a set of Rooms leads Sessions only
/// in those Rooms.
///
/// The search cannot violate this — it is a filter in
/// [`crate::solution::SearchState::statically_blocked`] — and this check
/// exists anyway, for ADR-0014's reason: an authoritative report that shares
/// no code with the occupancy index is the only thing that can catch the
/// index being wrong. Both sides call [`Problem::room_pin_blocks`], so the
/// solver cannot refuse a placement it then declines to report (ADR-0022).
///
/// PLACED placements only, mirroring [`lecturer_veto`] and [`group_veto`]
/// rather than `collect_views`. A locked Session's Room was chosen by the
/// caller, who has the app's own checker for it, and this rule's two nearest
/// siblings — same entity, same values-on-`Person` split — both report
/// placements only. (`Precedence` takes the other stance and counts locked
/// Sessions; consistency with the siblings wins here, and reversing it is a
/// `collect_views` swap in this one function.)
///
/// The detail names the PERSON and the ROOM, not just the Session: ADR-0027's
/// lesson that a report must name whoever declared the rule, since the
/// Session itself looks perfectly ordinary.
pub fn lecturer_room_pin(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    if problem.constraints.lecturer_room_pin.is_empty() {
        return;
    }
    for instance in &problem.constraints.lecturer_room_pin {
        for p in problem.placement_ids() {
            let Some(pl) = solution.get(p) else { continue };
            let o = problem.offering_of(p);
            if !instance.covers(&o.kind) {
                continue;
            }
            // The EFFECTIVE lecturer set: a pool Offering's choice lives on
            // the Placement, a fixed assignment's on the Offering, and
            // exactly one of the two is ever non-empty.
            let lecturers = || {
                o.lecturers
                    .iter()
                    .copied()
                    .chain(pl.lecturers.iter().flatten().copied())
            };
            if !problem.room_pin_blocks(lecturers(), pl.all_rooms()) {
                continue;
            }
            // Name the specific (Person, Room) pair that failed, rather than
            // reporting that something about this Session's rooms is wrong.
            let culprit = lecturers()
                .flat_map(|l| pl.all_rooms().map(move |r| (l, r)))
                .find(|&(l, r)| problem.room_pin_blocks(std::iter::once(l), std::iter::once(r)));
            let detail = match culprit {
                Some((l, r)) => format!(
                    "lecturer '{}' may not teach in room '{}'",
                    problem.persons[l.get()].id,
                    problem.rooms[r.get()].id
                ),
                None => "a pinned lecturer may not use this room".to_string(),
            };
            out.push(Violation {
                constraint_id: instance.id.clone(),
                constraint_type: ConstraintType::LecturerRoomPin,
                session_ids: vec![problem.placement_label(p)],
                offering_ids: vec![o.id.clone()],
                detail,
            });
        }
    }
}

/// HARD, unary. A lecturer is never assigned during their own blackout.
///
/// Evaluated over placed Sessions only: immovable occupancy is reported by the
/// caller's own data, and re-reporting a locked Session the solver cannot move
/// would be noise. Blackout VALUES live on `Person.blackouts`; the constraint
/// instance only switches enforcement on.
///
/// Asked against the placement's EFFECTIVE lecturers — a fixed assignment's
/// `Offering::lecturers` or a pool Offering's chosen `Placement::lecturers`,
/// exactly one of which is non-empty — through [`Problem::lecturer_veto_blocks`],
/// the same predicate the search filter's pool half uses (ADR-0014, ADR-0022).
/// NOT through `Offering::veto_slots`, which is empty for a pool: reading it
/// here would silently never report the case Calendry #131 exists for.
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
            let lecturers = || {
                o.lecturers
                    .iter()
                    .copied()
                    .chain(pl.lecturers.iter().flatten().copied())
            };
            for &s in &span {
                // Name the specific Person who is away, not just the Session:
                // a Session in a blackout looks perfectly ordinary on its own.
                let Some(culprit) =
                    lecturers().find(|&l| problem.lecturer_veto_blocks(std::iter::once(l), &[s]))
                else {
                    continue;
                };
                let f = problem.slots.flags(s);
                let who = problem.persons[culprit.get()].id.clone();
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
// SameTime / SameDays / SameStart — parallel Offering relations (issue #54)
//
// The opposite of `DifferentTime`: instead of no two members ever sharing a
// slot, every member's SAME-WEEK Sessions must agree on a SET (of days, of
// start blocks, or of `(day, block)` pairs, depending on kind). HARD, but
// PRICED at `hard_penalty` rather than enforced as an occupancy filter like
// `DifferentTime` — deliberately, not an oversight. A live `is_free`-style
// filter can only ever see PARTIAL state (a member's Sessions for a shared
// week accumulate one placement at a time, in whatever order construction
// or repair happens to try them), so "these two members' day-SETS are
// equal" is not decidable until both members' Sessions for that week are
// fully committed — there is no well-defined moment mid-search to check it
// as a filter without either rejecting on incomplete information or never
// refusing a genuine mismatch. Read fresh over every relation, the same
// "small enough to rescan" reasoning `imbalance_cost`/`max_days_violations`
// already rely on: relations and their members are few.
//
// PER-WEEK, BEST-EFFORT (RelationKind's own doc): a week where fewer than
// 2 members have a placed Session imposes no constraint, so this never
// requires members to share `required_session_count` — sidesteps the
// frequency-mismatch ill-definedness the tracking card itself flagged.
// ---------------------------------------------------------------------------

/// Every member's per-week key-SET, from PLACED Sessions only. `key_of`
/// picks the reduction: `iso_weekday` for `SameDays`, `block` for
/// `SameStart`, `(iso_weekday, block)` for `SameTime`. A week absent from
/// the result had fewer than 2 members with a placed Session — nothing to
/// compare, per the per-week-best-effort stance above.
fn member_week_sets<K: Eq + std::hash::Hash + Clone>(
    problem: &Problem,
    solution: &Solution,
    members: &[OfferingIdx],
    key_of: impl Fn(&crate::slots::SlotFlags) -> K,
) -> HashMap<u32, HashMap<OfferingIdx, HashSet<K>>> {
    let mut by_week: HashMap<u32, HashMap<OfferingIdx, HashSet<K>>> = HashMap::new();
    for p in problem.placement_ids() {
        let Some(pl) = solution.get(p) else { continue };
        let m = problem.placement(p).offering;
        if !members.contains(&m) {
            continue;
        }
        let f = problem.slots.flags(pl.start);
        by_week
            .entry(f.week)
            .or_default()
            .entry(m)
            .or_default()
            .insert(key_of(f));
    }
    by_week
}

/// Weeks where 2+ members have a placed Session AND their key-sets are not
/// all equal.
fn violated_weeks<K: Eq + std::hash::Hash>(
    sets: &HashMap<u32, HashMap<OfferingIdx, HashSet<K>>>,
) -> Vec<u32> {
    sets.iter()
        .filter(|(_, members_sets)| members_sets.len() >= 2)
        .filter(|(_, members_sets)| {
            let mut iter = members_sets.values();
            let first = iter.next().expect("filtered to len >= 2");
            iter.any(|s| s != first)
        })
        .map(|(&week, _)| week)
        .collect()
}

fn sorted<T: Ord + Copy>(s: &HashSet<T>) -> Vec<T> {
    let mut v: Vec<T> = s.iter().copied().collect();
    v.sort_unstable();
    v
}

pub fn same_time_violations(problem: &Problem, solution: &Solution) -> u32 {
    problem
        .relations
        .iter()
        .filter(|r| r.kind == RelationKind::SameTime)
        .map(|r| {
            let sets =
                member_week_sets(problem, solution, &r.members, |f| (f.iso_weekday, f.block));
            violated_weeks(&sets).len() as u32
        })
        .sum()
}

pub fn same_days_violations(problem: &Problem, solution: &Solution) -> u32 {
    problem
        .relations
        .iter()
        .filter(|r| r.kind == RelationKind::SameDays)
        .map(|r| {
            let sets = member_week_sets(problem, solution, &r.members, |f| f.iso_weekday);
            violated_weeks(&sets).len() as u32
        })
        .sum()
}

pub fn same_start_violations(problem: &Problem, solution: &Solution) -> u32 {
    problem
        .relations
        .iter()
        .filter(|r| r.kind == RelationKind::SameStart)
        .map(|r| {
            let sets = member_week_sets(problem, solution, &r.members, |f| f.block);
            violated_weeks(&sets).len() as u32
        })
        .sum()
}

/// Reports every currently-disagreeing `(relation, week)` for all three
/// kinds — a run can succeed while still naming which relation's week is
/// mismatched, the same "HARD but not filtered, so it must be reported"
/// stance `max_days`/`max_consecutive_days` already take.
pub fn same_relations(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    for r in problem
        .relations
        .iter()
        .filter(|r| r.kind == RelationKind::SameTime)
    {
        let sets = member_week_sets(problem, solution, &r.members, |f| (f.iso_weekday, f.block));
        for week in violated_weeks(&sets) {
            let members_sets = &sets[&week];
            let detail = members_sets
                .iter()
                .map(|(&o, s)| format!("'{}': {:?}", problem.offerings[o.get()].id, sorted(s)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(Violation {
                constraint_id: r.id.clone(),
                constraint_type: ConstraintType::SameTimeRelation,
                session_ids: Vec::new(),
                offering_ids: members_sets
                    .keys()
                    .map(|&o| problem.offerings[o.get()].id.clone())
                    .collect(),
                detail: format!(
                    "relation '{}' disagrees on (day, block) in week {week}: {detail}",
                    r.id
                ),
            });
        }
    }
    for r in problem
        .relations
        .iter()
        .filter(|r| r.kind == RelationKind::SameDays)
    {
        let sets = member_week_sets(problem, solution, &r.members, |f| f.iso_weekday);
        for week in violated_weeks(&sets) {
            let members_sets = &sets[&week];
            let detail = members_sets
                .iter()
                .map(|(&o, s)| format!("'{}': {:?}", problem.offerings[o.get()].id, sorted(s)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(Violation {
                constraint_id: r.id.clone(),
                constraint_type: ConstraintType::SameDaysRelation,
                session_ids: Vec::new(),
                offering_ids: members_sets
                    .keys()
                    .map(|&o| problem.offerings[o.get()].id.clone())
                    .collect(),
                detail: format!("relation '{}' disagrees on days in week {week}: {detail}", r.id),
            });
        }
    }
    for r in problem
        .relations
        .iter()
        .filter(|r| r.kind == RelationKind::SameStart)
    {
        let sets = member_week_sets(problem, solution, &r.members, |f| f.block);
        for week in violated_weeks(&sets) {
            let members_sets = &sets[&week];
            let detail = members_sets
                .iter()
                .map(|(&o, s)| format!("'{}': {:?}", problem.offerings[o.get()].id, sorted(s)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(Violation {
                constraint_id: r.id.clone(),
                constraint_type: ConstraintType::SameStartRelation,
                session_ids: Vec::new(),
                offering_ids: members_sets
                    .keys()
                    .map(|&o| problem.offerings[o.get()].id.clone())
                    .collect(),
                detail: format!(
                    "relation '{}' disagrees on start blocks in week {week}: {detail}",
                    r.id
                ),
            });
        }
    }
}

/// Reports a `MeetTogether` relation whose members disagree on (start, Room)
/// or whose combined size exceeds their shared Room's capacity, in a week
/// where 2+ of them have a placed Session.
///
/// `Occupancy::is_free` already prevents the search itself from ever
/// creating either — this exists for the same reason `DifferentTime`'s own
/// structural check exists despite being occupancy-backed too (ADR-0014):
/// a bad or stale LOCKED pairing bypasses the search's own filter entirely,
/// and needs its own independent check to be caught at all.
pub fn meet_together_relations(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    for r in problem
        .relations
        .iter()
        .filter(|r| r.kind == RelationKind::MeetTogether)
    {
        let mut by_week: HashMap<u32, Vec<(OfferingIdx, SlotIdx, RoomIdx, u32)>> = HashMap::new();
        for p in problem.placement_ids() {
            let Some(pl) = solution.get(p) else { continue };
            let m = problem.placement(p).offering;
            if !r.members.contains(&m) {
                continue;
            }
            let week = problem.slots.flags(pl.start).week;
            by_week.entry(week).or_default().push((
                m,
                pl.start,
                pl.room,
                problem.offerings[m.get()].min_capacity,
            ));
        }
        for (week, members) in &by_week {
            if members.len() < 2 {
                continue;
            }
            let (anchor_start, anchor_room) = (members[0].1, members[0].2);
            let names = || {
                members
                    .iter()
                    .map(|&(o, _, _, _)| problem.offerings[o.get()].id.clone())
                    .collect::<Vec<_>>()
            };
            let disagrees = members
                .iter()
                .any(|&(_, s, rm, _)| s != anchor_start || rm != anchor_room);
            if disagrees {
                let detail = members
                    .iter()
                    .map(|&(o, s, rm, _)| {
                        format!(
                            "'{}' at slot {} room '{}'",
                            problem.offerings[o.get()].id,
                            s.get(),
                            problem.rooms[rm.get()].id
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push(Violation {
                    constraint_id: r.id.clone(),
                    constraint_type: ConstraintType::MeetTogetherRelation,
                    session_ids: Vec::new(),
                    offering_ids: names(),
                    detail: format!(
                        "relation '{}' disagrees on (slot, room) in week {week}: {detail}",
                        r.id
                    ),
                });
                continue;
            }
            // Only meaningful once they agree — `capacity == 0` means
            // unbounded, the same reading `group_size_fits_room` gives it.
            let capacity = problem.rooms[anchor_room.get()].capacity;
            let combined: u32 = members.iter().map(|&(_, _, _, cap)| cap).sum();
            if capacity != 0 && combined > capacity {
                out.push(Violation {
                    constraint_id: r.id.clone(),
                    constraint_type: ConstraintType::MeetTogetherRelation,
                    session_ids: Vec::new(),
                    offering_ids: names(),
                    detail: format!(
                        "relation '{}' seats {combined} combined in week {week}, room '{}' seats only {capacity}",
                        r.id,
                        problem.rooms[anchor_room.get()].id
                    ),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Precedence — a lab must follow its lecture (issue #37)
//
// The first relation kind that reads member ORDER, which is the reason
// ADR-0028 made the set ordered rather than a bag. The members form a CHAIN:
// the rule binds each consecutive pair `(members[i], members[i+1])`.
//
// TERM-WIDE, ALL PAIRS: every Session of the predecessor must end before
// every Session of the successor begins — the block-teaching reading (all
// lectures finish before any lab starts). That reduces to ONE comparison per
// consecutive pair, because "every pair ordered" is exactly "the
// predecessor's LATEST end precedes the successor's EARLIEST start", so each
// pair has a single boundary and both parameters are measured across it
// rather than across n x m Session pairs.
//
// HARD, but PRICED at `hard_penalty` rather than filtered, the same stance
// `SameTime`/`SameDays`/`SameStart` and `MaxDays` take, for the same reason
// spelled out above them: the boundary is a property of two Offerings'
// COMPLETE placed sets, and no moment mid-construction has both. A candidate
// filter would have to either refuse on partial information or never refuse a
// genuine breach.
//
// LOCKED and PAST Sessions COUNT, unlike the `SameTime` family's placed-only
// `member_week_sets` above. Deliberate, and the divergence is the point: a
// repair run locks every out-of-scope Session, so a placed-only scan would
// make a relation whose predecessor is out of scope silently inert — the
// "enforces a DIFFERENT rule than the one configured" failure ADR-0028 names
// for dangling members, arrived at from the other direction.
// `DifferentTime` and `MeetTogether` already count locks (they read occupancy,
// and `FixedOccupancy` carries both relations' row lists); `SameTime`'s
// placed-only scan is the outlier, and is a per-week SET-equality question
// where counting a lock would force the search to match that lock's exact
// day and block — a stronger claim than the type makes. Ordering makes no
// such claim: a locked Session is simply one more Session in the order.
// ---------------------------------------------------------------------------

const MINUTES_PER_DAY: u32 = 24 * 60;

/// One end of a member's extent, in the three units the three checks need.
///
/// `block` and `minute` both increase with time and are never used
/// interchangeably, because they answer different questions and one of them
/// can be degenerate. **Ordering is decided on `block`**: it is structural,
/// exact, and defined for every grid. **The gap is measured in `minute`**,
/// because "at least a day between the lecture and the lab" is a wall-clock
/// claim that a block index cannot express. Deciding ordering on `minute`
/// instead would collapse a whole day into one instant on any grid whose
/// `block_length_minutes` is zero — which is what a caller sending no
/// wall-clock structure at all produces, and which must still order
/// correctly.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct Extreme {
    /// `week * 7 + (iso_weekday - 1)`, NOT [`crate::slots::SlotFlags::day_index`].
    /// `day_index` is dense over TEACHING days, so it would read a Friday →
    /// Monday boundary as one day apart and a closure week as no time at all.
    /// A tenant saying "within 2 days" means the student's calendar.
    day: u32,
    /// `day * blocks_per_day` plus the block index within the day — a total
    /// order over every slot in the grid, independent of any wall-clock
    /// configuration.
    block: u32,
    /// `day * MINUTES_PER_DAY` plus the minute-of-day, so a plain subtraction
    /// across any two Sessions is the true wall-clock gap. Resolved through
    /// [`crate::slots::GridTime`], which owns block lengths, the grid default
    /// gap and every named break.
    minute: u32,
}

/// A member's earliest start and latest end over the Sessions it currently
/// has, or `None` for a member with none at all — best-effort, imposing
/// nothing on the boundaries it participates in. Issue #37 asked for this
/// answer to be stated rather than left to throw or silently score zero.
#[derive(Copy, Clone, Debug)]
struct Extent {
    first_start: Extreme,
    last_end: Extreme,
}

impl Extent {
    /// Extremes are selected on `block`, never on `minute` — see [`Extreme`].
    fn widen(&mut self, start: Extreme, end: Extreme) {
        if start.block < self.first_start.block {
            self.first_start = start;
        }
        if end.block > self.last_end.block {
            self.last_end = end;
        }
    }
}

/// Why one boundary fails.
///
/// `OutOfOrder` excludes the other two: once the successor is already in the
/// wrong place, "the gap is 40 minutes short" and "these are 9 days apart"
/// are noise, not additional findings. The remaining two can co-fire, but
/// only under a self-contradicting configuration — a `min_gap_minutes` longer
/// than `max_days_between` leaves room for.
#[derive(Copy, Clone, Debug)]
enum Breach {
    /// The successor's first Session starts at or before the predecessor's
    /// last Session ends. The ordering itself, decided structurally.
    OutOfOrder,
    /// Correctly ordered, but closer together than `min_gap_minutes`.
    GapTooShort { observed: u32, required: u32 },
    /// Correctly ordered, but spanning FEWER calendar days than
    /// `min_days_between` requires.
    TooCloseInDays { observed: u32, required: u32 },
    /// Correctly ordered, but spanning more calendar days than
    /// `max_days_between` allows.
    TooFarApart { observed: u32, allowed: u32 },
}

fn calendar_day(f: &crate::slots::SlotFlags) -> u32 {
    f.week * 7 + f.iso_weekday - 1
}

/// Every member's [`Extent`], over placed AND fixed occupancy — see the
/// module section above for why locks count here.
fn precedence_extents(
    problem: &Problem,
    solution: &Solution,
    members: &[OfferingIdx],
) -> HashMap<OfferingIdx, Extent> {
    let mut out: HashMap<OfferingIdx, Extent> = HashMap::new();

    let blocks_per_day = problem.slots.blocks_per_day();
    let mut record = |m: OfferingIdx, start: SlotIdx, duration_blocks: u32| {
        if !members.contains(&m) {
            return;
        }
        let f = problem.slots.flags(start);
        let day = calendar_day(f);
        let last_block = f.block + duration_blocks.saturating_sub(1);
        let s = Extreme {
            day,
            block: day * blocks_per_day + f.block,
            minute: day * MINUTES_PER_DAY
                + problem.grid_time.block_start_minute(f.iso_weekday, f.block),
        };
        let e = Extreme {
            day,
            block: day * blocks_per_day + last_block,
            minute: day * MINUTES_PER_DAY
                + problem
                    .grid_time
                    .block_end_minute(f.iso_weekday, last_block),
        };
        out.entry(m)
            .and_modify(|x| x.widen(s, e))
            .or_insert(Extent { first_start: s, last_end: e });
    };

    for p in problem.placement_ids() {
        let Some(pl) = solution.get(p) else { continue };
        let m = problem.placement(p).offering;
        record(m, pl.start, problem.offerings[m.get()].duration_blocks);
    }
    for f in &problem.fixed {
        // An ad-hoc Session realizes no Offering, so it can be no relation's
        // member — the same `offering: None` reading every other
        // Offering-keyed check gives it.
        if let Some(m) = f.offering {
            record(m, f.start, f.duration_blocks);
        }
    }

    out
}

/// Walks every `Precedence` relation's consecutive-member boundaries and
/// hands each breach to `report`.
///
/// One walk behind both the counter and the reporter, so
/// [`precedence_violations`] and [`precedence_relations`] cannot drift: the
/// count IS the number of reported violations. Deterministic — relations in
/// configured order, members in configured order, and the two checks in a
/// fixed sequence — so it never iterates the `HashMap` it builds.
fn for_each_precedence_breach(
    problem: &Problem,
    solution: &Solution,
    mut report: impl FnMut(&crate::problem::RelationSpec, OfferingIdx, OfferingIdx, Breach),
) {
    for r in &problem.relations {
        let RelationKind::Precedence { min_gap_minutes, min_days_between, max_days_between } =
            r.kind
        else {
            continue;
        };
        let extents = precedence_extents(problem, solution, &r.members);
        for pair in r.members.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (Some(pred), Some(succ)) = (extents.get(&a), extents.get(&b)) else {
                continue;
            };
            // Ordering first, and exclusively: a boundary in the wrong order
            // has no meaningful gap to be short or long.
            if succ.first_start.block <= pred.last_end.block {
                report(r, a, b, Breach::OutOfOrder);
                continue;
            }
            // Ordered on `block`, so the day difference below is
            // non-negative by construction. The minute difference is too on
            // any sane grid, but saturates rather than trusting it: a grid
            // whose blocks run past midnight is a real state
            // (`block_end_minute` is documented as not wrapping at 24h), and
            // it would underflow here.
            let days = succ.first_start.day - pred.last_end.day;

            // The day FLOOR, and it excludes the minute gap the way the
            // ordering test above excludes both: a boundary landing on the
            // wrong DAY has no meaningful minute gap to be short. Without
            // the exclusion one mistake would be charged twice at
            // `hard_penalty` on a single boundary, since `Objective::hard`
            // sums the violation count and the count IS the number of
            // reports.
            //
            // It does NOT suppress the ceiling below: under contradictory
            // input (`min_days_between > max_days_between`) both bounds are
            // genuinely breached, and the timetabler should see both.
            if min_days_between > 0 && days < min_days_between {
                report(
                    r,
                    a,
                    b,
                    Breach::TooCloseInDays { observed: days, required: min_days_between },
                );
            } else {
                let gap = succ.first_start.minute.saturating_sub(pred.last_end.minute);
                if gap < min_gap_minutes {
                    report(
                        r,
                        a,
                        b,
                        Breach::GapTooShort { observed: gap, required: min_gap_minutes },
                    );
                }
            }

            if max_days_between > 0 && days > max_days_between {
                report(r, a, b, Breach::TooFarApart { observed: days, allowed: max_days_between });
            }
        }
    }
}

/// Breached `Precedence` boundaries, charged at `hard_penalty` like every
/// other HARD-but-priced term.
pub fn precedence_violations(problem: &Problem, solution: &Solution) -> u32 {
    let mut n = 0;
    for_each_precedence_breach(problem, solution, |_, _, _, _| n += 1);
    n
}

/// Reports every breached `Precedence` boundary, naming both Offerings and
/// what the boundary actually measured — a run can succeed while still saying
/// which lab landed before its lecture, the same stance `same_relations` and
/// `max_days` take.
pub fn precedence_relations(problem: &Problem, solution: &Solution, out: &mut Vec<Violation>) {
    for_each_precedence_breach(problem, solution, |r, a, b, breach| {
        let (before, after) = (&problem.offerings[a.get()].id, &problem.offerings[b.get()].id);
        let detail = match breach {
            Breach::OutOfOrder => {
                format!("relation '{}': '{after}' does not start after '{before}' ends", r.id)
            }
            Breach::GapTooShort { observed, required } => format!(
                "relation '{}': only {observed} minute(s) between '{before}' ending and \
                 '{after}' starting, below the required {required}",
                r.id
            ),
            Breach::TooCloseInDays { observed, required } => format!(
                "relation '{}': '{after}' starts {observed} day(s) after '{before}' ends, \
                 below the required {required}",
                r.id
            ),
            Breach::TooFarApart { observed, allowed } => format!(
                "relation '{}': '{after}' starts {observed} day(s) after '{before}' ends, \
                 above the allowed {allowed}",
                r.id
            ),
        };
        out.push(Violation {
            constraint_id: r.id.clone(),
            constraint_type: ConstraintType::PrecedenceRelation,
            session_ids: Vec::new(),
            offering_ids: vec![before.clone(), after.clone()],
            detail,
        });
    });
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
    //
    // A second exemption, for the same reason: two Sessions sharing a
    // Room via a common `MeetTogether` relation are not a clash either —
    // `Occupancy::is_free` already lets the search create exactly this, so
    // the authoritative checker must not then report it as one (ADR-0014).
    // `x`'s span already had to match `y`'s exactly for `Occupancy` to have
    // allowed this pairing, which is why span identity is not re-checked
    // here — a shared relation id at a shared slot is sufficient evidence.
    let meet_together_pair = x.room.is_some()
        && x.room == y.room
        && x.meet_together_relations
            .iter()
            .any(|r| y.meet_together_relations.contains(r));
    if let Some(r) = x.all_rooms().find(|&rx| y.all_rooms().any(|ry| ry == rx))
        && problem.rooms[r.get()].is_exclusive()
        && !meet_together_pair
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
    } else if let Some((rx, ry)) = x.all_rooms().find_map(|rx| {
        problem
            .footprint_siblings(rx)
            .iter()
            .find(|&&fp| y.all_rooms().any(|ry| ry == fp))
            .map(|&ry| (rx, ry))
    }) && !meet_together_pair
    {
        // DIFFERENT Rooms, one physical space — movable walls, where 1.0 and
        // the Audimax cannot both host a Session at one hour. Reported under
        // `RoomDoubleBooking` rather than a type of its own, because that is
        // what it is: the rule is unchanged, only the definition of "the same
        // room" widened. `footprint_siblings` already excludes non-exclusive
        // Rooms, so the exemption above needs no restating here.
        //
        // `else if`: an identical Room is the stronger statement and reads
        // better in a report, and a pair sharing both would otherwise be
        // named twice for one clash.
        //
        // Independent of `Occupancy`'s own check on purpose (ADR-0014). The
        // search can never produce this pair, but a caller's snapshot can —
        // two LOCKED Sessions either side of a folding wall — and the
        // authoritative checker is what tells the timetabler about it.
        for i in c.room_double_booking.iter().filter(|i| both(i)) {
            report(
                i,
                ConstraintType::RoomDoubleBooking,
                format!(
                    "rooms '{}' and '{}' share a physical footprint, and host '{}' and '{}' \
                     at {at}",
                    problem.rooms[rx.get()].id,
                    problem.rooms[ry.get()].id,
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
            meet_together_relations: &f.meet_together_relations,
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
            meet_together_relations: &o.meet_together_relations,
            span,
        });
    }

    views
}
