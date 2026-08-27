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
