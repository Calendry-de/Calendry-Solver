//! Run lifecycle.
//!
//! The solver owns run state for the lifetime of an in-progress run and nothing
//! longer: there is no database and no on-disk journal here. Nuxt polls
//! `GetStatus` and persists progress into its own `solver_run` table, which is
//! why unary calls suffice and no stream is held open.
//!
//! Four things about this module used to be narrower than its responsibility,
//! and each is now part of the interface rather than a convention:
//!
//! * **Status was an `i32`**, compared against `pb::RunStatus::X as i32` at six
//!   sites, and "is this state terminal" was hand-enumerated in two independent
//!   places. A new `RunStatus` in the pinned schema would silently have become
//!   *non*-terminal. It is now [`RunStatus`] with one [`RunStatus::is_terminal`].
//! * **The clock was ambient**, so no progress arithmetic was testable without
//!   sleeping. It is now injected — see [`crate::clock`].
//! * **Idempotency was racy**: `create` dropped the key guard, then re-locked to
//!   insert, so two concurrent `StartRun` with the same key both missed the
//!   lookup and both created a run. One lock over one `RegistryState` makes
//!   check-and-insert atomic.
//! * **Nothing was ever removed.** "Run state dies with the process" reads as a
//!   bound, but the interface had no word for "this run is over", so each
//!   terminal run retained its full `SolverOutput` — every placed Session of a
//!   27,136-Session university — for the process lifetime. [`Registry::reap`]
//!   gives the bound a name.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use calendry_solver_core::search::Halt;
use calendry_solver_proto::v1 as pb;

use crate::clock::{Clock, SystemClock};

/// Where a run is in its lifecycle.
///
/// Mirrors `pb::RunStatus` but as a Rust enum, so an exhaustive `match` is
/// available and "terminal" has exactly one definition.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum RunStatus {
    #[default]
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunStatus {
    /// Whether the run is finished and its status can no longer change.
    ///
    /// The single definition. `finish` and `cancel` each carried their own
    /// hand-enumerated copy of this list.
    pub fn is_terminal(self) -> bool {
        match self {
            Self::Succeeded | Self::Failed | Self::Cancelled => true,
            Self::Queued | Self::Running => false,
        }
    }

    /// The wire value. Exhaustive, so a new schema variant is a compile error
    /// here rather than a silent `0`.
    pub fn as_wire(self) -> i32 {
        let pb = match self {
            Self::Queued => pb::RunStatus::Queued,
            Self::Running => pb::RunStatus::Running,
            Self::Succeeded => pb::RunStatus::Succeeded,
            Self::Failed => pb::RunStatus::Failed,
            Self::Cancelled => pb::RunStatus::Cancelled,
        };
        pb as i32
    }
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub status: RunStatus,
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
    clock: Arc<dyn Clock>,
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
    fn new(
        id: String,
        seed: u64,
        max_wall_millis: u64,
        max_moves: u64,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let started = clock.now();
        Self {
            id,
            seed,
            clock,
            started,
            deadline: (max_wall_millis > 0)
                .then(|| started + Duration::from_millis(max_wall_millis)),
            max_moves,
            cancelled: AtomicBool::new(false),
            moves: AtomicU64::new(0),
            best_objective: AtomicU64::new(0),
            has_best: AtomicBool::new(false),
            state: Mutex::new(Progress { status: RunStatus::Queued, ..Default::default() }),
        }
    }

    fn elapsed(&self) -> Duration {
        self.clock.now().saturating_duration_since(self.started)
    }

    pub fn snapshot(&self) -> Progress {
        let mut p = self.state.lock().expect("run state poisoned").clone();
        if p.status == RunStatus::Running {
            p.elapsed_millis = self.elapsed().as_millis() as u64;
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
    pub fn progress_fraction(&self, elapsed_millis: u64, moves: u64) -> f64 {
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

    /// The wall-clock budget in milliseconds, or 0 if there is none.
    pub fn total_millis(&self) -> u64 {
        self.deadline
            .map_or(0, |d| d.duration_since(self.started).as_millis() as u64)
    }

    /// Publish an improved objective from the search thread.
    pub fn publish_best(&self, objective: f64, moves: u64) {
        self.best_objective
            .store(objective.to_bits(), Ordering::Relaxed);
        self.has_best.store(true, Ordering::Relaxed);
        self.moves.store(moves, Ordering::Relaxed);
    }

    pub fn elapsed_millis(&self) -> u64 {
        self.elapsed().as_millis() as u64
    }

    pub fn mark_running(&self) {
        let mut s = self.state.lock().expect("run state poisoned");
        s.status = RunStatus::Running;
    }

    pub fn finish(&self, status: RunStatus, result: Option<pb::SolverOutput>, error: String) {
        let mut s = self.state.lock().expect("run state poisoned");
        // A cancellation that lands while the search is finishing must not be
        // overwritten by a SUCCEEDED: the caller was told the run was cancelled.
        if s.status == RunStatus::Cancelled {
            return;
        }
        s.status = status;
        s.elapsed_millis = self.elapsed().as_millis() as u64;
        s.moves_evaluated = self.moves.load(Ordering::Relaxed);
        if status == RunStatus::Succeeded {
            s.progress = 1.0;
        }
        // The real weighted objective. NOTE: through slice 2 this field carried
        // the hard-violation count; see `docs/adr/` for the breaking change.
        s.best_objective = result
            .as_ref()
            .and_then(|r| r.objective.as_ref().map(|o| o.total))
            .unwrap_or(s.best_objective);
        s.result = result;
        s.error_detail = error;
    }

    /// Returns false if the run had already reached a terminal state.
    pub fn cancel(&self) -> (bool, RunStatus) {
        let mut s = self.state.lock().expect("run state poisoned");
        if s.status.is_terminal() {
            return (false, s.status);
        }
        self.cancelled.store(true, Ordering::SeqCst);
        s.status = RunStatus::Cancelled;
        s.elapsed_millis = self.elapsed().as_millis() as u64;
        (true, s.status)
    }

    pub fn record_moves(&self, n: u64) {
        self.moves.store(n, Ordering::Relaxed);
    }

    /// Whether this run has reached a terminal state, and how long ago.
    fn terminal_since(&self, now: Instant) -> Option<Duration> {
        let s = self.state.lock().expect("run state poisoned");
        if !s.status.is_terminal() {
            return None;
        }
        let finished_at = self.started + Duration::from_millis(s.elapsed_millis);
        Some(now.saturating_duration_since(finished_at))
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
            Some(d) if self.0.clock.now() >= d => Some("time_budget"),
            _ => None,
        }
    }
}

/// Everything the registry owns, behind **one** lock.
///
/// Two locks made `create`'s check-then-insert non-atomic, so the idempotency
/// key's whole promise — same key, same run — broke under exactly the
/// concurrency it exists for.
#[derive(Default)]
struct RegistryState {
    runs: HashMap<String, Arc<Run>>,
    idempotency: HashMap<String, String>,
}

pub struct Registry {
    state: Mutex<RegistryState>,
    clock: Arc<dyn Clock>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new(Arc::new(SystemClock))
    }
}

impl Registry {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self { state: Mutex::new(RegistryState::default()), clock }
    }

    /// Create a run, or return the existing one for a repeated idempotency key.
    ///
    /// The lookup and the insert happen under one guard, so two concurrent calls
    /// carrying the same key cannot both create a run.
    pub fn create(
        &self,
        seed: u64,
        max_wall_millis: u64,
        max_moves: u64,
        idempotency_key: &str,
    ) -> Arc<Run> {
        let mut state = self.state.lock().expect("registry poisoned");

        if !idempotency_key.is_empty()
            && let Some(existing) = state.idempotency.get(idempotency_key)
            && let Some(run) = state.runs.get(existing)
        {
            return run.clone();
        }

        let id = uuid::Uuid::new_v4().to_string();
        let run =
            Arc::new(Run::new(id.clone(), seed, max_wall_millis, max_moves, self.clock.clone()));

        state.runs.insert(id.clone(), run.clone());
        if !idempotency_key.is_empty() {
            state.idempotency.insert(idempotency_key.to_string(), id);
        }
        run
    }

    pub fn get(&self, id: &str) -> Option<Arc<Run>> {
        self.state
            .lock()
            .expect("registry poisoned")
            .runs
            .get(id)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.state.lock().expect("registry poisoned").runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop terminal runs that finished more than `retain_for` ago, along with
    /// the idempotency keys pointing at them. Returns how many were removed.
    ///
    /// This is the interface stating the bound that "no persistence beyond an
    /// in-progress run's lifetime" always implied. In-progress runs are never
    /// reaped, and a still-recent terminal run is kept so a caller polling
    /// `GetStatus` can collect its result.
    pub fn reap(&self, retain_for: Duration) -> usize {
        let now = self.clock.now();
        let mut state = self.state.lock().expect("registry poisoned");

        let expired: Vec<String> = state
            .runs
            .iter()
            .filter(|(_, run)| run.terminal_since(now).is_some_and(|d| d >= retain_for))
            .map(|(id, _)| id.clone())
            .collect();

        for id in &expired {
            state.runs.remove(id);
        }
        let RegistryState { runs, idempotency } = &mut *state;
        idempotency.retain(|_, id| runs.contains_key(id));
        expired.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TestClock;

    fn registry_with_clock() -> (Registry, Arc<TestClock>) {
        let clock = Arc::new(TestClock::new());
        (Registry::new(clock.clone()), clock)
    }

    // -- RunStatus -----------------------------------------------------------

    #[test]
    fn queued_and_running_are_not_terminal() {
        assert!(!RunStatus::Queued.is_terminal());
        assert!(!RunStatus::Running.is_terminal());
    }

    #[test]
    fn succeeded_failed_and_cancelled_are_terminal() {
        assert!(RunStatus::Succeeded.is_terminal());
        assert!(RunStatus::Failed.is_terminal());
        assert!(RunStatus::Cancelled.is_terminal());
    }

    #[test]
    fn wire_values_match_the_schema() {
        assert_eq!(RunStatus::Queued.as_wire(), pb::RunStatus::Queued as i32);
        assert_eq!(RunStatus::Running.as_wire(), pb::RunStatus::Running as i32);
        assert_eq!(RunStatus::Succeeded.as_wire(), pb::RunStatus::Succeeded as i32);
        assert_eq!(RunStatus::Failed.as_wire(), pb::RunStatus::Failed as i32);
        assert_eq!(RunStatus::Cancelled.as_wire(), pb::RunStatus::Cancelled as i32);
    }

    // -- progress arithmetic, asserted rather than slept ---------------------

    #[test]
    fn progress_is_zero_when_neither_budget_is_set() {
        let (registry, _clock) = registry_with_clock();
        let run = registry.create(1, 0, 0, "");
        assert_eq!(run.progress_fraction(500, 100), 0.0);
    }

    #[test]
    fn progress_follows_the_move_budget_when_only_moves_are_bounded() {
        let (registry, _clock) = registry_with_clock();
        let run = registry.create(1, 0, 200, "");
        assert_eq!(run.progress_fraction(0, 50), 0.25);
    }

    #[test]
    fn progress_follows_the_clock_when_only_time_is_bounded() {
        let (registry, _clock) = registry_with_clock();
        let run = registry.create(1, 1_000, 0, "");
        assert_eq!(run.progress_fraction(250, 0), 0.25);
    }

    #[test]
    fn progress_takes_whichever_budget_is_closer_to_exhaustion() {
        let (registry, _clock) = registry_with_clock();
        let run = registry.create(1, 1_000, 200, "");
        // 10% of the clock, 40% of the moves.
        assert_eq!(run.progress_fraction(100, 80), 0.4);
    }

    #[test]
    fn progress_never_exceeds_one_even_past_the_budget() {
        let (registry, _clock) = registry_with_clock();
        let run = registry.create(1, 1_000, 200, "");
        assert_eq!(run.progress_fraction(9_999, 9_999), 1.0);
    }

    #[test]
    fn elapsed_tracks_the_injected_clock() {
        let (registry, clock) = registry_with_clock();
        let run = registry.create(1, 0, 0, "");
        assert_eq!(run.elapsed_millis(), 0);
        clock.advance_millis(750);
        assert_eq!(run.elapsed_millis(), 750);
    }

    // -- the wall-clock halt, asserted rather than slept ---------------------

    #[test]
    fn the_wall_clock_halt_does_not_fire_before_the_deadline() {
        let (registry, clock) = registry_with_clock();
        let halt = RunHalt(registry.create(1, 1_000, 0, ""));
        clock.advance_millis(999);
        assert_eq!(halt.should_stop(), None);
    }

    #[test]
    fn the_wall_clock_halt_fires_once_the_deadline_passes() {
        let (registry, clock) = registry_with_clock();
        let halt = RunHalt(registry.create(1, 1_000, 0, ""));
        clock.advance_millis(1_000);
        assert_eq!(halt.should_stop(), Some("time_budget"));
    }

    #[test]
    fn an_unbudgeted_run_never_halts_on_time() {
        let (registry, clock) = registry_with_clock();
        let halt = RunHalt(registry.create(1, 0, 0, ""));
        clock.advance_millis(3_600_000);
        assert_eq!(halt.should_stop(), None);
    }

    #[test]
    fn cancellation_outranks_the_deadline() {
        let (registry, _clock) = registry_with_clock();
        let run = registry.create(1, 1_000, 0, "");
        let halt = RunHalt(run.clone());
        run.cancel();
        assert_eq!(halt.should_stop(), Some("cancelled"));
    }

    // -- lifecycle -----------------------------------------------------------

    #[test]
    fn cancelling_a_finished_run_reports_that_it_was_already_terminal() {
        let (registry, _clock) = registry_with_clock();
        let run = registry.create(1, 0, 0, "");
        run.finish(RunStatus::Succeeded, None, String::new());

        let (cancelled, status) = run.cancel();
        assert!(!cancelled);
        assert_eq!(status, RunStatus::Succeeded);
    }

    #[test]
    fn a_cancellation_is_not_overwritten_by_a_late_success() {
        // The search finishing after the caller was told the run was cancelled.
        let (registry, _clock) = registry_with_clock();
        let run = registry.create(1, 0, 0, "");
        run.mark_running();
        assert!(run.cancel().0);

        run.finish(RunStatus::Succeeded, None, String::new());
        assert_eq!(run.snapshot().status, RunStatus::Cancelled);
    }

    // -- idempotency ---------------------------------------------------------

    #[test]
    fn the_same_idempotency_key_returns_the_same_run() {
        let (registry, _clock) = registry_with_clock();
        let first = registry.create(1, 0, 0, "key-a");
        let second = registry.create(2, 0, 0, "key-a");
        assert_eq!(first.id, second.id);
        assert_eq!(registry.len(), 1, "no second run may be created");
    }

    #[test]
    fn an_empty_idempotency_key_never_dedupes() {
        let (registry, _clock) = registry_with_clock();
        let first = registry.create(1, 0, 0, "");
        let second = registry.create(2, 0, 0, "");
        assert_ne!(first.id, second.id);
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn concurrent_creates_with_one_key_produce_exactly_one_run() {
        // RED against the two-lock version, which dropped the key guard before
        // re-locking to insert: both threads missed the lookup, both created a
        // Run, and the second insert overwrote the mapping.
        let clock = Arc::new(TestClock::new());
        let registry = Arc::new(Registry::new(clock));

        let mut ids: Vec<String> = Vec::new();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let r = registry.clone();
                std::thread::spawn(move || r.create(i, 0, 0, "shared-key").id.clone())
            })
            .collect();
        for h in handles {
            ids.push(h.join().expect("worker panicked"));
        }
        assert_eq!(registry.len(), 1, "exactly one run may exist for one key");
        assert!(
            ids.windows(2).all(|w| w[0] == w[1]),
            "every caller must be handed the same run id, got {ids:?}"
        );
    }

    // -- retention -----------------------------------------------------------

    #[test]
    fn reaping_leaves_in_progress_runs_alone() {
        let (registry, clock) = registry_with_clock();
        let run = registry.create(1, 0, 0, "");
        run.mark_running();
        clock.advance_millis(60_000);

        assert_eq!(registry.reap(Duration::from_millis(1)), 0);
        assert!(registry.get(&run.id).is_some());
    }

    #[test]
    fn reaping_keeps_a_recently_finished_run_so_a_poll_can_collect_it() {
        let (registry, _clock) = registry_with_clock();
        let run = registry.create(1, 0, 0, "");
        run.finish(RunStatus::Succeeded, None, String::new());

        assert_eq!(registry.reap(Duration::from_secs(60)), 0);
        assert!(registry.get(&run.id).is_some());
    }

    #[test]
    fn reaping_drops_a_terminal_run_past_its_retention_and_its_key() {
        let (registry, clock) = registry_with_clock();
        let run = registry.create(1, 0, 0, "key-a");
        run.finish(RunStatus::Succeeded, None, String::new());
        clock.advance_millis(120_000);

        assert_eq!(registry.reap(Duration::from_secs(60)), 1);
        assert!(registry.get(&run.id).is_none());

        // The key must go with it, or a repeat would resolve to a run that is no
        // longer there and silently create a second one anyway.
        let fresh = registry.create(2, 0, 0, "key-a");
        assert_ne!(fresh.id, run.id);
        assert_eq!(registry.len(), 1);
    }
}
