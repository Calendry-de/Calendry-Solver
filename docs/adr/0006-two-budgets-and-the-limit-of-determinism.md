# Both a time budget and a move budget; whichever hits first ends the run

Both are configurable per request. A time budget is what an interactive caller
actually wants; a move budget is what makes a run reproducible.

**Same seed gives byte-identical output only when termination is itself
deterministic** — `"converged"`, `"stagnated"` or `"move_budget"`. A run
stopped by the wall-clock budget cannot be reproducible, because the number of
LNS iterations completed depends on machine speed and load. That is inherent to
a time-boxed metaheuristic, not a defect, and it is why `termination_reason`
exists: a caller can tell which guarantee they got.

`"stagnated"` joined the deterministic set with
[ADR-0031](0031-convergence-is-never-declared-over-unplaced-demand.md):
`"converged"` is reserved for a run whose best solution has zero unplaced
Sessions, and a run that exhausts the escalation ladder with demand still
unplaced reports `"stagnated"` — driven by the same counters and seeded RNG,
so the reproducibility contract is unchanged.

## Consequences

**Tests must use move budgets, never wall-clock budgets.** A determinism test
written against `max_wall_millis` will look flaky and will waste somebody's
afternoon. The shared test helpers in `crates/core/tests/common/mod.rs` exist to
make that choice visible at each call site.
