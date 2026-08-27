# The clock is behind a seam in the service, and finished runs are reaped

`Run` called `Instant::now()` in five places and computed its own deadline in its
constructor, so the clock was ambient *inside* the module with nothing in front
of it. Everything time-shaped was consequently unverifiable without sleeping: the
progress fraction's four-way match over `(by_time, by_moves)`, its clamp, and the
`time_budget` branch of `RunHalt::should_stop`.

The clock is now a one-method `Clock` trait with two adapters — `SystemClock` in
the binary, `TestClock` in the tests — which is what makes it a real seam rather
than indirection. ADR-0004's ban on a clock applies to **core**, because a run
must be reproducible from its seed; the service is where wall time legitimately
lives.

Two further gaps closed at the same time. The registry's idempotency check and
insert spanned **two locks**, so two concurrent `StartRun` calls carrying the same
key both missed the lookup and both created a run — breaking the key's promise
under exactly the concurrency it exists for. And nothing was ever removed:
ADR-0002 says run state dies with the process, which reads as a bound, but the
interface had no word for "this run is over", so every terminal run retained its
full `SolverOutput` — every placed Session of a 27,136-Session university — for
the process lifetime.

## Consequences

One lock over one `RegistryState` makes create atomic. `Registry::reap` gives the
retention bound a name; the binary runs it on a timer with a 15-minute retention,
long enough for a poller (ADR-0005) to collect a result.

## The reaper touches a cross-repo contract — read before changing the retention

The Nuxt side had been carrying this repo's unbounded-registry note in its own
`CLAUDE.md`, which meant the one repo that could fix it was the one repo that did
not know about it. It is fixed here, but the app depends on two behaviours that
reaping interacts with, so they change together or not at all:

* The app **captures a run's result the moment it goes terminal**, rather than
  when someone asks to apply it, because "I'll fetch it later" is a promise a
  restart breaks.
* The app treats `NOT_FOUND` as **terminal and unrecoverable** — the solver
  restarted and lost the run — while `UNAVAILABLE` is transient and leaves its
  row untouched.

The concern the app raised is real: an eviction policy makes `NOT_FOUND` mean two
things, and the app cannot tell them apart. What makes it tolerable rather than
breaking is the *order* of the two behaviours above. Because the app captures on
terminal, a reaped run is one whose result the app already holds, and the
retention window is the margin that guarantees it: 15 minutes, against a poll
interval measured in seconds.

So the rule is a floor on the retention, not a ban on reaping. **Retention must
stay comfortably longer than the app's longest plausible gap between a run going
terminal and its poller noticing.** Shortening it toward that gap re-creates the
ambiguity with no warning, because nothing in either repo tests the interaction.

Still worth confirming from the app side before the retention is tuned: this ADR
records the reasoning, not agreement.
