# The benchmark generator and the correctness fixtures do not share a source of truth

Correctness fixtures are hand-written and checked in, in
`crates/core/src/testing.rs`. Benchmark instances come from a parametrized
generator in `crates/gen`. The two never share a source of truth.

The reason is direct: a generator bug that produced a wrong fixture would be a
bug that **silently validates itself**. The generator's tests accordingly assert
only that instances are well-formed and reproducible — never that the solver's
answer on one of them is right.

## Consequences

They may share an *assembly interface* without sharing a source of truth, and
they do: both build a `ProblemSpec` and call `Problem::build`, so closure and
attendee semantics cannot drift between them. Each still chooses its own values.
