//! Targeted-repair churn measurement.
//!
//! Answers, empirically, the question `In-scope Sessions have no stay-put
//! pressure` (calendry solver issue #58) explicitly says not to guess at:
//! when a scope names one Offering to fix a single forced conflict, how many
//! of that Offering's OTHER Sessions move anyway, with nothing preferring
//! their current slot over any other?
//!
//! Method: solve a real generated term whole, pick the Offering with the most
//! Sessions and none of its own already fixed, then rebuild a second `Problem`
//! containing ONLY that Offering as a movable `PlacementVar` — every other
//! Offering's baseline placement becomes a locked, `offering: None` ad-hoc
//! occupant (same shape the wire already uses for staff meetings), so Room/
//! Group/Person/lecturer occupancy is exactly as tight as the real term's,
//! without needing to reconstruct every other Offering. One of the target's
//! own Sessions is then forced to be infeasible at its baseline slot+room by
//! an anonymous room-closure occupant — the "week-7 clash" — and the
//! narrowed problem is solved again from scratch. Every OTHER target Session
//! that lands somewhere different than its baseline slot+room is churn.
use calendry_solver_core::ids::{OfferingIdx, PlacementIdx};
use calendry_solver_core::problem::{
    FixedSpec, Immovable, OfferingSpec, PlacementVar, Problem, ProblemSpec, ScopeSpec,
};
use calendry_solver_core::search::{self, Budget, NeverHalt};
use calendry_solver_core::solution::{MAX_ADDITIONAL_ROOMS, Placement};

#[derive(Debug, Clone)]
pub struct ChurnReport {
    pub target_offering: String,
    /// Sessions of the target Offering other than the one forced to move —
    /// or, in the `with_clash: false` control, every one of its Sessions,
    /// since none is forced.
    pub free_placements: usize,
    /// How many of `free_placements` landed away from their baseline slot+room.
    pub churned: usize,
    pub churn_ratio: f64,
}

/// Run both the real scenario (one forced clash) and the control (an
/// identical narrowed re-solve with NOTHING forced to move at all), so the
/// second isolates how much of the first's churn is gratuitous — the search
/// re-optimizing Sessions nobody asked it to touch — rather than caused by
/// resolving the one real conflict.
pub fn measure_with_control(
    problem: &Problem,
    seed: u64,
    budget: Budget,
) -> Option<(ChurnReport, ChurnReport)> {
    Some((measure(problem, seed, budget, true)?, measure(problem, seed, budget, false)?))
}

/// `None` when no generated Offering is both large enough and free of its own
/// fixed Sessions — the measurement needs a clean `required_session_count ==
/// placement_count` Offering to rebuild without disturbing `ExactFrequency`.
pub fn measure(
    problem: &Problem,
    seed: u64,
    budget: Budget,
    with_clash: bool,
) -> Option<ChurnReport> {
    let baseline = search::solve(problem, seed, budget, &NeverHalt);

    let target = problem
        .offering_ids()
        .filter(|&o| problem.immovable_count(o) == 0 && problem.placement_count(o) >= 8)
        .max_by_key(|&o| problem.placement_count(o))?;

    let target_placements: Vec<PlacementIdx> = problem
        .placement_ids()
        .filter(|&p| problem.placement(p).offering == target)
        .collect();

    let baseline_at = |p: PlacementIdx| -> Placement {
        baseline
            .solution
            .get(p)
            .expect("baseline solve places every Session")
    };

    let forced = target_placements[0];
    let forced_at = baseline_at(forced);
    let target_offering = &problem.offerings[target.get()];

    let mut fixed: Vec<FixedSpec> = problem
        .offering_ids()
        .filter(|&o| o != target)
        .flat_map(|o| {
            problem
                .placement_ids()
                .filter(move |&p| problem.placement(p).offering == o)
        })
        .map(|p| {
            let o = problem.placement(p).offering;
            let offering = &problem.offerings[o.get()];
            let at = baseline_at(p);
            FixedSpec {
                session_id: format!("locked-{}", p.get()),
                offering: None,
                kind: offering.kind.clone(),
                room: Some(at.room),
                additional_rooms: at.additional_rooms,
                start: at.start,
                duration_blocks: offering.duration_blocks,
                lecturers: offering.lecturers.clone(),
                groups: offering.own_groups.clone(),
                persons: offering.participants.clone(),
                reason: Immovable::Locked,
            }
        })
        .collect();

    // The forced clash: an anonymous closure occupying the target's own
    // baseline room at its own baseline slot, for its own duration — so
    // exactly this one occurrence is infeasible where it was, and nothing
    // else is. Omitted entirely in the control run.
    if with_clash {
        fixed.push(FixedSpec {
            session_id: "forced-closure".to_string(),
            offering: None,
            kind: "closure".to_string(),
            room: Some(forced_at.room),
            additional_rooms: [None; MAX_ADDITIONAL_ROOMS],
            start: forced_at.start,
            duration_blocks: target_offering.duration_blocks,
            lecturers: vec![],
            groups: vec![],
            persons: vec![],
            reason: Immovable::Locked,
        });
    }

    let offerings = vec![OfferingSpec {
        id: target_offering.id.clone(),
        kind: target_offering.kind.clone(),
        required_session_count: target_offering.required_session_count,
        duration_blocks: target_offering.duration_blocks,
        lecturers: target_offering.lecturers.clone(),
        eligible_lecturer_combinations: target_offering.eligible_lecturer_combinations.clone(),
        groups: target_offering.own_groups.clone(),
        participants: target_offering.participants.clone(),
        eligible_rooms: target_offering.eligible_rooms.clone(),
        required_room_count: target_offering.required_room_count,
        eligible_room_combinations: target_offering.eligible_room_combinations.clone(),
        min_capacity: target_offering.min_capacity,
        scheduling_pattern: target_offering.scheduling_pattern,
    }];

    let placements: Vec<PlacementVar> = (0..target_placements.len())
        .map(|occurrence| PlacementVar {
            offering: OfferingIdx(0),
            occurrence: occurrence as u32,
            existing_session_id: None,
            original: None,
        })
        .collect();

    let narrowed = Problem::build(ProblemSpec {
        rooms: problem.rooms.clone(),
        groups: problem.groups.clone(),
        persons: problem.persons.clone(),
        offerings,
        placements,
        fixed,
        constraints: problem.constraints.clone(),
        scope: ScopeSpec::Offerings(vec![OfferingIdx(0)]),
        ..ProblemSpec::new(problem.slots.clone())
    })
    .expect("rebuilt from an already-valid group hierarchy");

    let repaired = search::solve(&narrowed, seed, budget, &NeverHalt);

    // With a forced clash, index 0 (`forced`) is excluded from the count —
    // it was NEVER going to land back where it started, so counting it would
    // conflate "had to move" with "moved gratuitously". The control has no
    // forced Session, so nothing is excluded.
    let start = if with_clash { 1 } else { 0 };
    let churned = (start..target_placements.len())
        .filter(|&i| {
            let before = baseline_at(target_placements[i]);
            let after = repaired.solution.get(PlacementIdx(i as u32));
            after != Some(before)
        })
        .count();

    let free_placements = target_placements.len() - start;
    Some(ChurnReport {
        target_offering: target_offering.id.clone(),
        free_placements,
        churned,
        churn_ratio: if free_placements == 0 {
            0.0
        } else {
            churned as f64 / free_placements as f64
        },
    })
}
