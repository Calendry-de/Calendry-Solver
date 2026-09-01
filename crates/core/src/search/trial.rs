//! [`Trial`]: solution, incremental index and objective, kept in agreement
//! **by construction**. See the struct's own doc for the history that made
//! this a module invariant rather than caller bookkeeping.

use crate::constraints;
use crate::ids::PlacementIdx;
use crate::problem::Problem;
use crate::soft::Objective;
use crate::solution::{Placement, SearchState, Solution};

use super::construction::construct;
use super::objective::{objectives_agree, recompute_objective};

/// One change to the trial, enough to reverse it.
#[derive(Copy, Clone, Debug)]
enum Change {
    Placed(PlacementIdx, Placement),
    Removed(PlacementIdx, Placement),
}

/// The scalar half of the objective, restored verbatim on rollback.
///
/// Snapshotting rather than replaying the arithmetic backwards is deliberate:
/// `f64` addition is not associative, so `(a - x) + x` is not guaranteed to be
/// `a`, and a rejected round must leave the accepted objective bit-identical.
#[derive(Copy, Clone, Debug)]
struct Scalars {
    unplaced: u32,
    soft: f64,
    journal_len: usize,
}

/// Solution, incremental index and objective, kept in agreement **by
/// construction**.
///
/// `solve` used to hand-maintain the three as siblings, and every mutation had
/// to update all three, in the right order, at four separate sites — with the
/// undo reversing two of them by one mechanism and the third by a different one
/// (not assigning it). The project knew this was fragile: there was a
/// per-iteration `debug_assert` comparing the incremental objective against a
/// full recomputation, and its comment called it "the classic metaheuristic
/// bug". That assertion was the honest admission that the invariant had no
/// owner. It also cost a public interface, because `recompute_objective` and
/// `objectives_agree` had to be exported for tests to re-run the same check.
///
/// Now every primitive updates all three together, so agreement is structural.
/// The drift check remains, as [`Trial::assert_consistent`], but it verifies one
/// module's invariant instead of cross-checking a caller's bookkeeping.
pub struct Trial<'p> {
    problem: &'p Problem,
    solution: Solution,
    state: SearchState,
    unplaced: u32,
    soft: f64,
    /// LIFO record of what the open round changed.
    journal: Vec<Change>,
    open: Option<Scalars>,
}

impl<'p> Trial<'p> {
    /// Greedy construction, with the objective computed once from scratch.
    pub fn construct(problem: &'p Problem) -> Self {
        let (solution, state) = construct(problem);
        let objective = recompute_objective(problem, &solution);
        Self {
            problem,
            solution,
            state,
            unplaced: objective.unplaced,
            soft: objective.soft,
            journal: Vec::new(),
            open: None,
        }
    }

    #[inline]
    pub fn solution(&self) -> &Solution {
        &self.solution
    }

    #[inline]
    pub fn state(&self) -> &SearchState {
        &self.state
    }

    /// The current objective.
    ///
    /// `unplaced` and `soft` are maintained incrementally. `aggregate` and
    /// `day_mix_cost` are read straight off the counters, which ARE the running
    /// state rather than something a delta accumulates into — a violated share
    /// cell and a mixed `(group, day)` cell each belong to no single placement,
    /// so neither can be attributed to one as a delta.
    #[inline]
    pub fn objective(&self) -> Objective {
        Objective {
            unplaced: self.unplaced,
            aggregate: self.state.share_violations(),
            max_days_violations: self.state.max_days_violations(),
            max_consecutive_days_violations: self.state.max_consecutive_days_violations(),
            same_time_violations: constraints::same_time_violations(self.problem, &self.solution),
            same_days_violations: constraints::same_days_violations(self.problem, &self.solution),
            same_start_violations: constraints::same_start_violations(self.problem, &self.solution),
            precedence_violations: constraints::precedence_violations(self.problem, &self.solution),
            soft: self.soft,
            day_mix_cost: self.state.day_mix_cost(self.problem),
            compactness_cost: self.state.compactness_cost(self.problem),
            max_consecutive_cost: self.state.max_consecutive_cost(self.problem),
            max_daily_span_cost: self.state.max_daily_span_cost(self.problem),
            max_daily_session_count_cost: self.state.max_daily_session_count_cost(self.problem),
            max_weekly_teaching_load_cost: self.state.max_weekly_teaching_load_cost(self.problem),
            exam_same_day_cost: self.state.exam_same_day_cost(self.problem),
            exam_window_cost: self.state.exam_window_cost(self.problem),
            imbalance_cost: self.state.imbalance_cost(self.problem),
            location_change_cost: self.state.location_change_cost(self.problem),
            room_turnaround_cost: self.state.room_turnaround_cost(self.problem),
            room_churn_cost: self.state.room_churn_cost(self.problem),
            room_consistency_cost: self.state.room_consistency_cost(self.problem),
            lecturer_consistency_cost: self.state.lecturer_consistency_cost(self.problem),
            offering_daily_count_cost: self.state.offering_daily_count_cost(self.problem),
            offering_run_cost: self.state.offering_run_cost(self.problem),
            offering_split_cost: self.state.offering_split_cost(self.problem),
            scheduling_pattern_cost: self.state.scheduling_pattern_cost(self.problem),
            daybreak_cost: self.state.daybreak_cost(self.problem),
            travel_cost: self.state.travel_cost(self.problem),
            offering_distinct_days_cost: self.state.offering_distinct_days_cost(self.problem),
        }
    }

    /// Place `p` at `at`, updating solution, index and objective together.
    ///
    /// Returns `false` and changes nothing if the Session would not fit the grid
    /// there — the case the open-coded ritual used to skip silently while
    /// recording the placement anyway.
    #[must_use = "a false return means nothing was placed"]
    pub fn place(&mut self, p: PlacementIdx, at: Placement) -> bool {
        debug_assert!(self.solution.get(p).is_none(), "place on an occupied placement");
        if !self.state.place(self.problem, p, at) {
            return false;
        }
        self.solution.set(p, Some(at));
        let o = self.problem.offering_of(p);
        let capacity = self.problem.exclusive_capacity(at.all_rooms());
        self.soft += at
            .all_rooms()
            .map(|r| self.problem.soft.cost(o.soft_profile, at.start, r))
            .sum::<f64>()
            + self.problem.preference_cost_for_placement(o, p, at)
            + self.problem.movement_cost(p, at.start, at.room)
            + self.problem.capacity_waste_cost(o, capacity)
            + self.problem.specialized_room_cost(o, at.all_rooms())
            + self
                .problem
                .break_spanning_cost(o, at.start, o.duration_blocks);
        self.unplaced -= 1;
        self.journal.push(Change::Placed(p, at));
        true
    }

    /// Remove `p`, returning where it was. `None` if it was already unplaced,
    /// in which case nothing changed.
    pub fn unplace(&mut self, p: PlacementIdx) -> Option<Placement> {
        let at = self.solution.get(p)?;
        let released = self.state.unplace(self.problem, p, at);
        debug_assert!(released, "a placed Session's span must still resolve");
        self.solution.set(p, None);
        let o = self.problem.offering_of(p);
        let capacity = self.problem.exclusive_capacity(at.all_rooms());
        self.soft -= at
            .all_rooms()
            .map(|r| self.problem.soft.cost(o.soft_profile, at.start, r))
            .sum::<f64>()
            + self.problem.preference_cost_for_placement(o, p, at)
            + self.problem.movement_cost(p, at.start, at.room)
            + self.problem.capacity_waste_cost(o, capacity)
            + self.problem.specialized_room_cost(o, at.all_rooms())
            + self
                .problem
                .break_spanning_cost(o, at.start, o.duration_blocks);
        self.unplaced += 1;
        self.journal.push(Change::Removed(p, at));
        Some(at)
    }

    /// Start recording, so [`Trial::rollback`] can reverse exactly what follows.
    pub fn begin(&mut self) {
        debug_assert!(self.open.is_none(), "a round is already open");
        self.open = Some(Scalars {
            unplaced: self.unplaced,
            soft: self.soft,
            journal_len: self.journal.len(),
        });
    }

    /// Keep everything the open round did.
    pub fn commit(&mut self) {
        let Some(mark) = self.open.take() else { return };
        self.journal.truncate(mark.journal_len);
    }

    /// Reverse the open round exactly, in O(k).
    pub fn rollback(&mut self) {
        let Some(mark) = self.open.take() else { return };
        while self.journal.len() > mark.journal_len {
            // LIFO: undoing in reverse order means the index sees the same
            // sequence of marks it would have seen had the round never run.
            match self.journal.pop() {
                Some(Change::Placed(p, at)) => {
                    let released = self.state.unplace(self.problem, p, at);
                    debug_assert!(released, "rollback of a placement that did not resolve");
                    self.solution.set(p, None);
                }
                Some(Change::Removed(p, at)) => {
                    let marked = self.state.place(self.problem, p, at);
                    debug_assert!(marked, "rollback of a removal that did not resolve");
                    self.solution.set(p, Some(at));
                }
                None => break,
            }
        }
        // Restored verbatim, not recomputed: see [`Scalars`].
        self.unplaced = mark.unplaced;
        self.soft = mark.soft;
    }

    /// The maintained objective must equal a from-scratch recomputation.
    ///
    /// Delta drift is the classic metaheuristic bug — the search optimizes a
    /// number that has quietly diverged from the real objective. Checked on
    /// every iteration in debug builds, and by an explicit test.
    #[inline]
    pub fn assert_consistent(&self) {
        debug_assert!(
            objectives_agree(self.objective(), recompute_objective(self.problem, &self.solution)),
            "incremental objective {:?} diverged from recomputed {:?}",
            self.objective(),
            recompute_objective(self.problem, &self.solution)
        );
    }
}
