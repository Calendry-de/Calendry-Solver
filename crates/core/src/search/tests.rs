use crate::aggregates::ShareWindow;
use crate::ids::{PlacementIdx, RoomIdx, SlotIdx};
use crate::problem::Problem;
use crate::solution::{Placement, SearchState, Solution};
use crate::testing;

use super::construction::construct;
use super::ruin::ruin_worst;
use super::trial::Trial;

/// ADR-0025's falsification target: before the fix, `ruin_worst` ranked by
/// placement-local `soft` alone, which is blind to a `MaxOnlineShare`
/// breach. Four equally-costed (zero soft) Sessions of one Group at a 50%
/// cap, with the **on-site** one placed at the LOWEST index and the three
/// online ones after it — deliberately, so the old scoring's tie-break
/// ("descending cost, ties by ascending index") is put to the test rather
/// than dodged by accident.
///
/// Old scoring: every placement costs 0.0 soft, so the tie-break alone
/// decides and picks placement 0 — the on-site one. That pick cannot
/// repair the breach: removing the on-site Session only shrinks the
/// denominator, which cannot lower the online count back under the
/// allowance. New scoring must instead score the three online placements
/// above zero (they sit in the one violated cell) and the on-site one at
/// zero, so `k=1` must return one of the online placements.
#[test]
fn ruin_worst_prefers_an_online_placement_in_a_breaching_share_cell() {
    let problem =
        testing::share_capped_group(vec![testing::share_rule("s", 0.5, ShareWindow::PerTerm)]);

    let mut solution = Solution::empty(&problem);
    let mut state = SearchState::from_fixed(&problem);

    // Placement 0 on-site (room 1), placements 1..3 online (room 0) —
    // on-site sits at the lowest index on purpose (see doc comment).
    let placements = [
        Placement::single(SlotIdx(0), RoomIdx(1)),
        Placement::single(SlotIdx(1), RoomIdx(0)),
        Placement::single(SlotIdx(2), RoomIdx(0)),
        Placement::single(SlotIdx(3), RoomIdx(0)),
    ];
    let placed: Vec<PlacementIdx> = (0..4).map(PlacementIdx).collect();
    for (&p, &pl) in placed.iter().zip(&placements) {
        assert!(state.place(&problem, p, pl), "fixture placement must resolve");
        solution.set(p, Some(pl));
    }
    assert_eq!(state.share_violations(), 1, "3 of 4 online is 75% > the 50% cap");

    // Ruining only the on-site placement can never fix the breach. The
    // old lowest-index tie-break would have returned exactly `[0]` here;
    // the fixed scoring must not.
    let chosen = ruin_worst(&problem, &solution, &state, &placed, 1);
    assert_eq!(chosen.len(), 1);
    assert_ne!(
        chosen[0],
        PlacementIdx(0),
        "removing the on-site Session cannot repair an online-share breach"
    );
    assert!(
        [PlacementIdx(1), PlacementIdx(2), PlacementIdx(3)].contains(&chosen[0]),
        "must pick one of the three online placements sitting in the breaching cell"
    );
}

/// A cell that is not violated must contribute nothing, so `ruin_worst`
/// falls back to soft cost (here, an explicit tie) rather than being
/// permanently biased toward whichever placements happen to be online.
#[test]
fn ruin_worst_is_blind_to_online_placements_outside_any_breach() {
    let problem =
        testing::share_capped_group(vec![testing::share_rule("s", 1.0, ShareWindow::PerTerm)]);

    let mut solution = Solution::empty(&problem);
    let mut state = SearchState::from_fixed(&problem);
    let placements = [
        Placement::single(SlotIdx(0), RoomIdx(0)),
        Placement::single(SlotIdx(1), RoomIdx(1)),
    ];
    let placed: Vec<PlacementIdx> = (0..2).map(PlacementIdx).collect();
    for (&p, &pl) in placed.iter().zip(&placements) {
        assert!(state.place(&problem, p, pl));
        solution.set(p, Some(pl));
    }
    assert_eq!(state.share_violations(), 0, "100% cap permits any mix");

    // Both cost zero soft and zero aggregate, so the tie-break must still
    // be the deterministic lowest-index rule `ruin_worst` documents.
    let chosen = ruin_worst(&problem, &solution, &state, &placed, 1);
    assert_eq!(chosen, vec![PlacementIdx(0)]);
}

// -------------------------------------------------------------------
// Minimize-movement (LOCK_POLICY_MINIMIZE_MOVEMENT)
// -------------------------------------------------------------------

/// A single movable, OUT-OF-SCOPE placement, `original` set to
/// `(orig_slot, orig_room)`. Bypasses `testing::assemble`, which calls
/// `expand_placements` and would overwrite `original` with `None` —
/// exactly the v1 shape these tests are testing past. Out of scope is
/// what selects `movement_weight` in `Problem::movement_cost`.
fn movable_problem(rooms_n: u32, eligible: &[u32], original: (u32, u32)) -> Problem {
    use crate::ids::OfferingIdx;
    use crate::problem::{PlacementVar, ProblemSpec, ScopeSpec};
    let (orig_slot, orig_room) = original;
    let spec = ProblemSpec {
        rooms: testing::rooms(rooms_n),
        offerings: vec![testing::offering("o", 1, eligible)],
        placements: vec![PlacementVar {
            offering: OfferingIdx(0),
            occurrence: 0,
            existing_session_id: Some("s1".into()),
            original: Some((SlotIdx(orig_slot), Some(RoomIdx(orig_room)))),
        }],
        movement_weight: 1.0,
        scope: ScopeSpec::Offerings(vec![]),
        ..ProblemSpec::new(testing::grid(4, 1))
    };
    Problem::build(spec).unwrap()
}

#[test]
fn construction_places_a_movable_session_back_at_its_original_slot_and_room() {
    let problem = movable_problem(1, &[0], (2, 0));
    let (solution, _) = construct(&problem);
    assert_eq!(
        solution.get(PlacementIdx(0)),
        Some(Placement::single(SlotIdx(2), RoomIdx(0))),
        "nothing else competes for this slot, so construction must not \
         gratuitously charge the movement penalty for no reason"
    );
}

#[test]
fn construction_does_not_reuse_an_original_room_the_offering_no_longer_considers_eligible() {
    // Room 0 was the original, but the Offering's eligibility was
    // redefined to room 1 only. Trying the original blindly would place a
    // Session in a room its own Offering does not consider eligible —
    // bypassing the eligibility filter is not a smaller sin just because
    // minimize-movement asked for it.
    let problem = movable_problem(2, &[1], (2, 0));
    let (solution, _) = construct(&problem);
    assert_eq!(
        solution.get(PlacementIdx(0)),
        Some(Placement::single(SlotIdx(0), RoomIdx(1))),
        "must fall through to the ordinary greedy scan — earliest feasible \
         slot, only eligible room — not the ineligible original room"
    );
}

#[test]
fn ruin_worst_picks_up_a_movement_charge() {
    use crate::ids::OfferingIdx;
    use crate::problem::{PlacementVar, ProblemSpec, ScopeSpec};

    // Placement 0: ordinary, no `original`, free wherever it sits.
    // Placement 1: movable, `original` at slot 2, but PLACED at slot 1 —
    // displaced, so it alone carries a nonzero movement cost. Deliberately
    // the HIGHER index, so a tie-break-by-ascending-index would pick
    // placement 0 — only reading the movement cost into the score can
    // make this test pick placement 1 instead.
    let spec = ProblemSpec {
        rooms: testing::rooms(1),
        offerings: vec![testing::offering("o", 2, &[0])],
        placements: vec![
            PlacementVar {
                offering: OfferingIdx(0),
                occurrence: 0,
                existing_session_id: None,
                original: None,
            },
            PlacementVar {
                offering: OfferingIdx(0),
                occurrence: 1,
                existing_session_id: Some("s1".into()),
                original: Some((SlotIdx(2), Some(RoomIdx(0)))),
            },
        ],
        movement_weight: 1.0,
        scope: ScopeSpec::Offerings(vec![]),
        ..ProblemSpec::new(testing::grid(4, 1))
    };
    let problem = Problem::build(spec).unwrap();

    let mut solution = Solution::empty(&problem);
    let mut state = SearchState::from_fixed(&problem);
    let placements = [
        Placement::single(SlotIdx(0), RoomIdx(0)),
        Placement::single(SlotIdx(1), RoomIdx(0)),
    ];
    let placed: Vec<PlacementIdx> = (0..2).map(PlacementIdx).collect();
    for (&p, &pl) in placed.iter().zip(&placements) {
        assert!(state.place(&problem, p, pl));
        solution.set(p, Some(pl));
    }

    let chosen = ruin_worst(&problem, &solution, &state, &placed, 1);
    assert_eq!(
        chosen,
        vec![PlacementIdx(1)],
        "the displaced movable placement must outrank the free ordinary one"
    );
}

/// The classic metaheuristic bug ADR-0026/ADR-0025 both guard against:
/// `Trial::place`/`unplace` maintain `soft` as a delta, and a term added at
/// only some of the read sites would quietly diverge from a from-scratch
/// recomputation. Exercises `place`, `unplace` and `assert_consistent`
/// together with a NONZERO movement cost, which the fixture in
/// `movable_problem` never produces on its own.
#[test]
fn incremental_objective_matches_full_recomputation_with_movement_cost() {
    let problem = movable_problem(1, &[0], (2, 0));

    let mut trial = Trial::construct(&problem);
    trial.assert_consistent();

    let at = trial
        .unplace(PlacementIdx(0))
        .expect("construction must have placed it");
    assert_eq!(at, Placement::single(SlotIdx(2), RoomIdx(0)), "back at its original");
    trial.assert_consistent();

    // Force it away from `original`, so the movement term is actually
    // nonzero for the rest of this check.
    let moved = Placement::single(SlotIdx(0), RoomIdx(0));
    assert!(trial.place(PlacementIdx(0), moved));
    trial.assert_consistent();
}
