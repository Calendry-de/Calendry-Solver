# Architecture decision records

One file per decision, numbered sequentially. Each records *that* a decision was
made and *why* — not a specification. Domain vocabulary lives in
[`CONTEXT.md`](../../CONTEXT.md); measurements live in
[`PERFORMANCE.md`](../PERFORMANCE.md).

These are decisions, not suggestions. Before changing something one of them
covers, read it: several exist specifically because the obvious thing was tried
and measured to be wrong.

## Architecture

| # | Decision |
|---|---|
| [0001](0001-hybrid-heuristic-plus-local-search.md) | Hybrid constructive heuristic plus local search, CPU-only for v1 |
| [0002](0002-no-database-stateless-over-grpc.md) | The solver never touches Postgres |
| [0003](0003-proto-schema-as-a-pinned-submodule.md) | The protobuf schema lives in a separate repo, consumed as a pinned submodule |
| [0004](0004-four-crates-core-must-not-see-prost.md) | Four crates, so that `core` cannot see prost types |
| [0005](0005-unary-rpcs-and-solver-owned-run-state.md) | Unary RPCs only; the solver owns in-flight run state and the app polls |
| [0006](0006-two-budgets-and-the-limit-of-determinism.md) | Both a time budget and a move budget; whichever hits first ends the run |
| [0031](0031-convergence-is-never-declared-over-unplaced-demand.md) | Convergence is never declared over unplaced demand |

## Domain and constraints

| # | Decision |
|---|---|
| [0007](0007-fourteen-typed-constraint-types-no-dsl.md) | Predefined constraint types, each a compiled evaluator; no expression DSL |
| [0008](0008-one-solve-mechanism-scope-plus-lock-policy.md) | One solve mechanism: a scope plus a lock policy |
| [0015](0015-getstatus-best-objective-carries-the-weighted-objective.md) | `GetStatus.best_objective` carries the weighted objective, not a violation count |
| [0016](0016-scope-membership-is-carried-not-inferred.md) | Scope membership is carried into `Problem`, not inferred from placement presence |
| [0022](0022-a-virtual-room-is-not-an-exclusive-resource.md) | A virtual Room is not an exclusive resource |
| [0023](0023-onlineonsitesameday-is-priced-not-forbidden.md) | `OnlineOnsiteSameDay` is priced, not forbidden |
| [0024](0024-one-type-per-axis-with-flags.md) | One constraint type per axis, with flags, rather than one type per direction |
| [0025](0025-maxonlineshare-is-not-enforced-by-the-search.md) | `MaxOnlineShare` is not enforced by the search, and never was |
| [0026](0026-personpreferencefit-charges-the-unmet-fraction.md) | `PersonPreferenceFit` charges the unmet fraction, and is not a `SoftParams` variant |
| [0027](0027-group-blackouts-inherit-downward.md) | Group blackouts inherit downward, and the query walks up |
| [0028](0028-a-relation-is-an-ordered-set-of-offerings.md) | A distribution relation is an ordered set of Offerings plus a type |
| [0029](0029-candidates-are-independent-seeded-runs-not-a-batch-rpc.md) | Multiple candidates are independent seeded runs, filtered for distance — not a batch RPC |
| [0030](0030-a-rotating-block-pattern-decomposes-into-parts-that-already-exist.md) | A rotating block pattern decomposes into three parts, two of which already exist |

## Benchmarking

| # | Decision |
|---|---|
| [0009](0009-generator-separate-from-correctness-fixtures.md) | The benchmark generator and the correctness fixtures do not share a source of truth |
| [0010](0010-calibrate-on-the-binding-axis.md) | Benchmark instances are calibrated on the binding axis, not on room tightness |
| [0011](0011-a-load-metric-cannot-certify-feasibility.md) | A load metric cannot certify feasibility: the person-clique bound |
| [0012](0012-electives-are-root-level-groups.md) | Electives are root-level Groups with their own Offerings |

## Module shape

| # | Decision |
|---|---|
| [0013](0013-move-evaluation-behind-a-trait.md) | Move evaluation sits behind a trait, for a future GPU backend |
| [0014](0014-structural-stays-independent-of-occupancy.md) | The authoritative structural check stays independent of the occupancy index |
| [0017](0017-conversion-errors-are-typed-transport-mapping-is-one-place.md) | Conversion errors are typed; the mapping to gRPC lives in one place |
| [0018](0018-the-service-crate-has-a-library-target.md) | The service crate has a library target |
| [0019](0019-the-clock-is-behind-a-seam-in-the-service.md) | The clock is behind a seam in the service, and finished runs are reaped |

## Process

| # | Decision |
|---|---|
| [0020](0020-workspace-lints-and-ci-are-the-gate.md) | Lints are configured once in the workspace manifest, and CI is the gate |
| [0021](0021-measure-end-to-end-before-optimizing-a-component.md) | Measure end-to-end impact before optimizing a diagnosed component |
