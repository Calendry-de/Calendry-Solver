//! Constraint evaluators.
//!
//! One typed, compiled function per constraint type. There is no interpreter
//! and no expression language: tenant-supplied logic never executes. Adding a
//! type is a code change here, by design.
//!
//! The v1 slice implements exactly two, and the pairing is deliberate. Room
//! double-booking alone is not falsifiable: with nothing forcing placement, the
//! objective-optimal solution is to place nothing, which satisfies it
//! vacuously. Exact frequency supplies the placement pressure that makes room
//! double-booking a real constraint.

use crate::problem::Problem;
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
pub const EXACT_FREQUENCY: &str = "ExactFrequency";

/// Evaluate every enabled hard constraint over a complete solution.
///
/// Deterministic ordering: constraints in a fixed sequence, and within each,
/// entities in index order. Two runs with the same seed must produce
/// byte-identical violation lists, so this must never iterate a HashMap.
pub fn evaluate_hard(problem: &Problem, solution: &Solution) -> Vec<Violation> {
    let mut out = Vec::new();
    if let Some(id) = &problem.constraints.exact_frequency {
        exact_frequency(problem, solution, id, &mut out);
    }
    if let Some(id) = &problem.constraints.room_double_booking {
        room_double_booking(problem, solution, id, &mut out);
    }
    out
}

/// HARD. Each in-scope Offering must be realized by exactly
/// `required_session_count` placed Sessions.
fn exact_frequency(
    problem: &Problem,
    solution: &Solution,
    constraint_id: &str,
    out: &mut Vec<Violation>,
) {
    let mut placed = vec![0u32; problem.offerings.len()];
    for p in problem.placement_ids() {
        if solution.get(p).is_some() {
            placed[problem.placement(p).offering.get()] += 1;
        }
    }

    for (i, offering) in problem.offerings.iter().enumerate() {
        // An Offering with no placement variables is out of scope for this run;
        // its frequency is not this run's business.
        let in_scope = problem
            .placements
            .iter()
            .any(|pv| pv.offering.get() == i);
        if !in_scope {
            continue;
        }

        let want = offering.required_session_count;
        let got = placed[i];
        if got != want {
            out.push(Violation {
                constraint_id: constraint_id.to_string(),
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

/// HARD. A Room hosts at most one Session per slot.
///
/// Considers placed Sessions *and* fixed occupancy, so a caller snapshot that
/// already double-books a room — which the app's "warn and allow" manual-edit
/// UX permits — is reported rather than silently tolerated.
fn room_double_booking(
    problem: &Problem,
    solution: &Solution,
    constraint_id: &str,
    out: &mut Vec<Violation>,
) {
    // slot -> room -> occupants. Kept as a flat Vec keyed by (room, slot) so
    // iteration order is deterministic.
    let n_slots = problem.slots.len();
    let mut occupants: Vec<Vec<String>> = vec![Vec::new(); problem.rooms.len() * n_slots];

    let mut mark = |room: usize, slot: usize, who: String| {
        occupants[room * n_slots + slot].push(who);
    };

    for f in &problem.fixed {
        let Some(room) = f.room else { continue };
        if let Some(span) = problem.slots.span(f.start, f.duration_blocks) {
            for s in span {
                mark(room.get(), s.get(), f.session_id.clone());
            }
        }
    }

    for p in problem.placement_ids() {
        let Some(pl) = solution.get(p) else { continue };
        let offering = problem.offering_of(p);
        let who = problem
            .placement(p)
            .existing_session_id
            .clone()
            .unwrap_or_else(|| format!("{}#{}", offering.id, problem.placement(p).occurrence));
        if let Some(span) = problem.slots.span(pl.start, offering.duration_blocks) {
            for s in span {
                mark(pl.room.get(), s.get(), who.clone());
            }
        }
    }

    for room in 0..problem.rooms.len() {
        for slot in 0..n_slots {
            let cell = &occupants[room * n_slots + slot];
            if cell.len() > 1 {
                let f = problem.slots.flags(crate::ids::SlotIdx(slot as u32));
                out.push(Violation {
                    constraint_id: constraint_id.to_string(),
                    constraint_type: ROOM_DOUBLE_BOOKING,
                    session_ids: cell.clone(),
                    offering_ids: Vec::new(),
                    detail: format!(
                        "room '{}' has {} sessions at week {} day {} block {}",
                        problem.rooms[room].id,
                        cell.len(),
                        f.week,
                        f.iso_weekday,
                        f.block
                    ),
                });
            }
        }
    }
}
