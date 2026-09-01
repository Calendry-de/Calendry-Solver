//! Greedy construction — the deterministic, seed-independent first phase.

use crate::ids::PlacementIdx;
use crate::problem::Problem;
use crate::solution::{Occupant, Placement, SearchState, Solution};

/// Greedy construction: most-constrained-first, first feasible slot.
pub fn construct(problem: &Problem) -> (Solution, SearchState) {
    let mut solution = Solution::empty(problem);
    let mut state = SearchState::from_fixed(problem);

    // Ordering is a pure function of the problem; the seed only ever perturbs
    // the LNS phase, so construction is reproducible on its own.
    let mut order: Vec<PlacementIdx> = problem.placement_ids().collect();
    order.sort_by(|&a, &b| {
        let (oa, ob) = (problem.offering_of(a), problem.offering_of(b));
        oa.eligible_rooms
            .len()
            .cmp(&ob.eligible_rooms.len())
            .then(ob.attendees.len().cmp(&oa.attendees.len()))
            .then(ob.duration_blocks.cmp(&oa.duration_blocks))
            .then(a.cmp(&b))
    });

    for p in order {
        let offering = problem.offering_of(p);
        let base = Occupant::of_offering(offering);

        // Testing the room-independent axes ONCE per slot, before the room loop,
        // is a pure short-circuit: if they reject, no Room can rescue the slot,
        // so the loop that follows could only have failed. Measured, ~60% of
        // start slots are rejected this way, and the saving is larger than that
        // count suggests — the room check is a single early-exiting bit test,
        // while the room-independent path scans an attendee list averaging 65
        // people. Previously that scan ran once per *free* Room per slot.
        //
        // The mask itself lives on `Occupant`, because the benchmark harness's
        // construction attribution has to use the identical one to report
        // truthfully. See [`Occupant::room_independent_probe`].
        let slot_probe = Occupant::room_independent_probe(offering);

        let mut chosen = None;

        // A movable out-of-scope Session (`LOCK_POLICY_MINIMIZE_MOVEMENT`)
        // already has a place it belongs; try exactly that first so
        // construction does not gratuitously charge the movement penalty for
        // a Session nothing has yet asked to move. Falling back to a
        // DIFFERENT eligible room here would still count as "moved" by
        // `Problem::movement_cost`, so there is no cheaper substitute worth
        // trying before the general scan below — only the exact original
        // counts as free.
        //
        // Gated on the original room still being ELIGIBLE for this Offering:
        // a Session's Offering can be redefined after it was scheduled (a lab
        // reassigned away from a room it used to be eligible for), and the
        // room-eligibility filter is a business rule the search must never
        // bypass, minimize-movement or not. An ineligible original falls
        // through to the general scan below, which prices the resulting
        // move — correctly, since it genuinely cannot stay.
        //
        // Also gated on NOT having a lecturer pool: this fast path tries
        // exactly one candidate, which is only meaningful when there is
        // exactly one lecturer choice to go with it. A pool Offering falls
        // through to the general scan, which tries every lecturer
        // combination at every slot — losing the "prefer the original slot"
        // shortcut for the rare pool-plus-minimize-movement case rather than
        // complicating this single-candidate path with a second loop.
        if !offering.has_lecturer_pool()
            && let Some((orig_start, Some(orig_room))) = problem.placement(p).original
            && offering.eligible_rooms.contains(&orig_room)
            && let Some(span) = problem.slots.span(orig_start, offering.duration_blocks)
        {
            let candidate = base.with_room(orig_room);
            if state.is_free(problem, &candidate, &span) {
                chosen = Some(Placement::single(orig_start, orig_room));
            }
        }

        if chosen.is_none() {
            'search: for slot in problem.slots.all() {
                let Some(span) = problem.slots.span(slot, offering.duration_blocks) else {
                    continue;
                };
                if let Some(probe) = slot_probe.as_ref()
                    && !state.is_free(problem, probe, &span)
                {
                    continue;
                }
                for i in 0..offering.room_choice_count() {
                    let (room, additional_rooms) = offering.room_choice(i);
                    for j in 0..offering.lecturer_choice_count() {
                        let lecturers = offering.lecturer_choice(j);
                        let candidate = base
                            .with_room(room)
                            .with_additional_rooms(additional_rooms)
                            .with_pool_lecturers(lecturers);
                        if state.is_free(problem, &candidate, &span) {
                            chosen =
                                Some(Placement { start: slot, room, additional_rooms, lecturers });
                            break 'search;
                        }
                    }
                }
            }
        }

        // Leaving a placement unplaced is a legitimate outcome: it surfaces as
        // an ExactFrequency violation rather than an error, because the solver
        // must degrade gracefully on infeasible input.
        if let Some(placement) = chosen {
            let marked = state.place(problem, p, placement);
            debug_assert!(marked, "construction chose a placement whose span resolved");
            solution.set(p, Some(placement));
        }
    }

    (solution, state)
}
