//! Run lifecycle.
//!
//! The solver owns run state for the lifetime of an in-progress run and nothing
//! longer: there is no database and no on-disk journal here. Nuxt polls
//! `GetStatus` and persists progress into its own `solver_run` table, which is
//! why unary calls suffice and no stream is held open.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use calendry_solver_core::search::Halt;
use calendry_solver_proto::v1 as pb;

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub status: i32,
    pub progress: f64,
    pub best_objective: f64,
    pub moves_evaluated: u64,
    pub elapsed_millis: u64,
    pub result: Option<pb::SolverOutput>,
    pub error_detail: String,
}

pub struct Run {
    pub id: String,
    pub seed: u64,
    started: Instant,
    /// `None` = no wall-clock budget.
    deadline: Option<Instant>,
    /// 0 = no move budget.
    max_moves: u64,
    cancelled: AtomicBool,
    moves: AtomicU64,
    /// Best objective so far, as `f64` bits so the search can publish it
    /// without taking the state lock on every improvement.
    best_objective: AtomicU64,
    has_best: AtomicBool,
    state: Mutex<Progress>,
}

impl Run {
    fn new(id: String, seed: u64, max_wall_millis: u64, max_moves: u64) -> Self {
        let started = Instant::now();
        Self {
            id,
            seed,
            started,
            deadline: (max_wall_millis > 0)
                .then(|| started + std::time::Duration::from_millis(max_wall_millis)),
            max_moves,
            cancelled: AtomicBool::new(false),
            moves: AtomicU64::new(0),
            best_objective: AtomicU64::new(0),
            has_best: AtomicBool::new(false),
            state: Mutex::new(Progress {
                status: pb::RunStatus::Queued as i32,
                ..Default::default()
            }),
        }
    }

    pub fn snapshot(&self) -> Progress {
        let mut p = self.state.lock().unwrap().clone();
        if p.status == pb::RunStatus::Running as i32 {
            p.elapsed_millis = self.started.elapsed().as_millis() as u64;
            p.moves_evaluated = self.moves.load(Ordering::Relaxed);
            p.progress = self.progress_fraction(p.elapsed_millis, p.moves_evaluated);
            if self.has_best.load(Ordering::Relaxed) {
                p.best_objective = f64::from_bits(self.best_objective.load(Ordering::Relaxed));
            }
        }
        p
    }

    /// Best-effort, as the proto documents: how far through whichever budget is
    /// closest to being exhausted. With both budgets unbounded there is nothing
    /// to measure against, so it stays 0 until the run is terminal.
    fn progress_fraction(&self, elapsed_millis: u64, moves: u64) -> f64 {
        let by_time = self.deadline.map(|_| {
            let total = self.total_millis();
            if total == 0 { 0.0 } else { elapsed_millis as f64 / total as f64 }
        });
        let by_moves = (self.max_moves > 0).then(|| moves as f64 / self.max_moves as f64);
        match (by_time, by_moves) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => 0.0,
        }
        .clamp(0.0, 1.0)
    }

    fn total_millis(&self) -> u64 {
        self.deadline
            .map(|d| d.duration_since(self.started).as_millis() as u64)
            .unwrap_or(0)
    }

    /// Publish an improved objective from the search thread.
    pub fn publish_best(&self, objective: f64, moves: u64) {
        self.best_objective
            .store(objective.to_bits(), Ordering::Relaxed);
        self.has_best.store(true, Ordering::Relaxed);
        self.moves.store(moves, Ordering::Relaxed);
    }

    pub fn elapsed_millis(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn mark_running(&self) {
        let mut s = self.state.lock().unwrap();
        s.status = pb::RunStatus::Running as i32;
    }

    pub fn finish(&self, status: pb::RunStatus, result: Option<pb::SolverOutput>, error: String) {
        let mut s = self.state.lock().unwrap();
        // A cancellation that lands while the search is finishing must not be
        // overwritten by a SUCCEEDED: the caller was told the run was cancelled.
        if s.status == pb::RunStatus::Cancelled as i32 {
            return;
        }
        s.status = status as i32;
        s.elapsed_millis = self.started.elapsed().as_millis() as u64;
        s.moves_evaluated = self.moves.load(Ordering::Relaxed);
        s.progress = if status == pb::RunStatus::Succeeded { 1.0 } else { s.progress };
        // The real weighted objective. NOTE: through slice 2 this field carried
        // the hard-violation count; see CLAUDE.md for the breaking change.
        s.best_objective = result
            .as_ref()
            .and_then(|r| r.objective.as_ref().map(|o| o.total))
            .unwrap_or(s.best_objective);
        s.result = result;
        s.error_detail = error;
    }

    /// Returns false if the run had already reached a terminal state.
    pub fn cancel(&self) -> (bool, i32) {
        let mut s = self.state.lock().unwrap();
        let terminal = s.status == pb::RunStatus::Succeeded as i32
            || s.status == pb::RunStatus::Failed as i32
            || s.status == pb::RunStatus::Cancelled as i32;
        if terminal {
            return (false, s.status);
        }
        self.cancelled.store(true, Ordering::SeqCst);
        s.status = pb::RunStatus::Cancelled as i32;
        s.elapsed_millis = self.started.elapsed().as_millis() as u64;
        (true, s.status)
    }

    pub fn record_moves(&self, n: u64) {
        self.moves.store(n, Ordering::Relaxed);
    }
}

/// Bridges cancellation and the wall-clock budget into the search loop.
///
/// The move-count budget is enforced inside the search itself, since only the
/// search knows how many moves it has evaluated; both budgets apply and
/// whichever is hit first ends the run.
pub struct RunHalt(pub Arc<Run>);

impl Halt for RunHalt {
    fn report(&self, objective: f64, moves: u64) {
        self.0.publish_best(objective, moves);
    }

    fn should_stop(&self) -> Option<&'static str> {
        if self.0.cancelled.load(Ordering::Relaxed) {
            return Some("cancelled");
        }
        match self.0.deadline {
            Some(d) if Instant::now() >= d => Some("time_budget"),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Registry {
    runs: Mutex<HashMap<String, Arc<Run>>>,
    idempotency: Mutex<HashMap<String, String>>,
}

impl Registry {
    pub fn create(
        &self,
        seed: u64,
        max_wall_millis: u64,
        max_moves: u64,
        idempotency_key: &str,
    ) -> Arc<Run> {
        if !idempotency_key.is_empty() {
            let keys = self.idempotency.lock().unwrap();
            if let Some(existing) = keys.get(idempotency_key)
                && let Some(run) = self.runs.lock().unwrap().get(existing)
            {
                return run.clone();
            }
        }

        let id = uuid::Uuid::new_v4().to_string();
        let run = Arc::new(Run::new(id.clone(), seed, max_wall_millis, max_moves));

        self.runs.lock().unwrap().insert(id.clone(), run.clone());
        if !idempotency_key.is_empty() {
            self.idempotency
                .lock()
                .unwrap()
                .insert(idempotency_key.to_string(), id);
        }
        run
    }

    pub fn get(&self, id: &str) -> Option<Arc<Run>> {
        self.runs.lock().unwrap().get(id).cloned()
    }
}
