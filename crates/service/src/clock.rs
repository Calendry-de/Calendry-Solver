//! The clock, behind a seam.
//!
//! `Run` called `Instant::now()` in five places and computed its own deadline in
//! its constructor, so the clock was *ambient inside* the module with nothing in
//! front of it. Everything time-shaped was consequently unverifiable without
//! sleeping: `progress_fraction`'s four-way match over `(by_time, by_moves)`, its
//! `clamp`, `total_millis`'s zero fallback, and the `time_budget` branch of
//! `RunHalt::should_stop`.
//!
//! The ban on a clock applies to **core** (ADR-0004), because a run must be
//! reproducible from its seed. The service is where wall time legitimately
//! lives, so this is the right home for the seam. See
//! `docs/adr/0019-the-clock-is-behind-a-seam-in-the-service.md`.
//!
//! Two adapters, which is what makes it a real seam rather than indirection:
//! [`SystemClock`] in the binary, [`TestClock`] in the tests.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A source of monotonic time.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Wall time. The production adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A clock that only moves when a test moves it.
///
/// Lets the progress arithmetic and the wall-clock halt be *asserted* rather
/// than slept through.
#[derive(Debug)]
pub struct TestClock {
    origin: Instant,
    offset: Mutex<Duration>,
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

impl TestClock {
    pub fn new() -> Self {
        Self { origin: Instant::now(), offset: Mutex::new(Duration::ZERO) }
    }

    /// Move time forward.
    pub fn advance(&self, by: Duration) {
        *self.offset.lock().expect("test clock poisoned") += by;
    }

    /// Move time forward by whole milliseconds.
    pub fn advance_millis(&self, millis: u64) {
        self.advance(Duration::from_millis(millis));
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        self.origin + *self.offset.lock().expect("test clock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_clock_does_not_move_on_its_own() {
        let clock = TestClock::new();
        assert_eq!(clock.now(), clock.now());
    }

    #[test]
    fn advancing_a_test_clock_moves_it_by_exactly_that_much() {
        let clock = TestClock::new();
        let before = clock.now();
        clock.advance_millis(250);
        assert_eq!(clock.now().duration_since(before), Duration::from_millis(250));
    }
}
