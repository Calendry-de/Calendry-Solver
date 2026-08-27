# calendry-solver

Rust optimization service for **Calendry**, a multi-tenant timetabling platform.
This service does the scheduling maths; the Nuxt app owns everything else.

## Where things are written down

This file is a **map and a checklist**, not the record. It used to hold
everything, which meant a reader had to scroll past benchmark tables to find the
domain model and past decisions to find how to run the tests. The content is now
split by what kind of thing it is:

| | |
|---|---|
| [`CONTEXT.md`](CONTEXT.md) | **The glossary.** Domain vocabulary — Offering, Session, TimeGrid, conflict closure — plus the architecture vocabulary used when designing modules. No implementation details. |
| [`docs/adr/`](docs/adr/) | **The decisions**, one per file, with why. Several exist because the obvious thing was tried and measured to be wrong — read the relevant one before changing what it covers. [Index.](docs/adr/README.md) |
| [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) | **The measurements.** Phase timings, what was fixed and what it was worth, and what was instrumented and deliberately not done. |
| [`docs/SCHEMA.md`](docs/SCHEMA.md) | **Schema distribution status**, and what remains in the Nuxt repo. |

Nothing in those files can be re-derived from code or git history. If a fact
there conflicts with an assumption you are about to make, the file wins.

The formats those files use are not invented here. `.claude/skills/` carries the
skills that define them, installed project-level and pinned by content hash in
`skills-lock.json`:

| Skill | What it is for |
|---|---|
| `domain-modeling` | The `CONTEXT.md` and ADR formats, and when an ADR is worth writing at all |
| `codebase-design` | The architecture vocabulary in `CONTEXT.md` — module, interface, depth, seam, adapter |
| `improve-codebase-architecture` | Scanning for deepening opportunities and reporting them |
| `grilling`, `grill-with-docs` | Stress-testing a design, writing the docs as decisions crystallize |
| `rust-best-practices` | Apollo's Rust handbook, referenced by the lint policy in ADR-0020 |
| `graft` | This repo's code graph — query it before grepping |

Update them with `npx skills update -p`. Two flags are easy to get wrong:
`-a claude-code`, not `claude`, and `-s` must be **repeated** per skill rather
than given a comma-separated list.

---

## Non-negotiables

Check any change against these. Each links to the decision behind it.

- [ ] No Postgres, no DB driver, no persistence beyond in-flight run state — [ADR-0002](docs/adr/0002-no-database-stateless-over-grpc.md)
- [ ] No `.proto` files in this repo; consume `calendry-proto` as a pinned submodule — [ADR-0003](docs/adr/0003-proto-schema-as-a-pinned-submodule.md)
- [ ] No prost, tonic, tokio, I/O or clock in `crates/core` — [ADR-0004](docs/adr/0004-four-crates-core-must-not-see-prost.md)
- [ ] No timeslot arithmetic hardcoding day or week structure; resolve against the TimeGrid
- [ ] No exam-week or holiday logic by array slicing; resolve against the Academic Calendar
- [ ] No per-person timezone anywhere in grid or constraint logic
- [ ] No expression evaluation of tenant-supplied strings; typed parameters only — [ADR-0007](docs/adr/0007-fourteen-typed-constraint-types-no-dsl.md)
- [ ] Room exclusivity is read from `Room::is_exclusive()`, never from a room id — [ADR-0022](docs/adr/0022-a-virtual-room-is-not-an-exclusive-resource.md)
- [ ] A preset never moves to make a violation count fall — [ADR-0025](docs/adr/0025-maxonlineshare-is-not-enforced-by-the-search.md)
- [ ] Past Sessions excluded from recalculation, always — [ADR-0008](docs/adr/0008-one-solve-mechanism-scope-plus-lock-policy.md)
- [ ] Locked and out-of-scope Sessions never moved (v1: hard lock) — [ADR-0008](docs/adr/0008-one-solve-mechanism-scope-plus-lock-policy.md)
- [ ] Group conflict checks use precomputed ancestor+descendant sets, never a live tree walk
- [ ] Move evaluation stays behind the trait boundary — [ADR-0013](docs/adr/0013-move-evaluation-behind-a-trait.md)
- [ ] The solver tolerates infeasible input; the app's "warn and allow" UX produces it
- [ ] Tests use move budgets, never wall-clock budgets — [ADR-0006](docs/adr/0006-two-budgets-and-the-limit-of-determinism.md)

The nested-group rule is a performance requirement as much as a correctness one:
conflict propagation is evaluated in the local-search hot loop, potentially
millions of times per run. The app side separately uses a closure table for the
same rule — same semantics, different representation, and they must agree.

---

## Repository layout

| Path | Crate | Role | Must not depend on |
|---|---|---|---|
| `crates/core` | `calendry-solver-core` | Domain model, dense indices, slot tables, evaluators, search | prost, tonic, tokio, I/O, any clock |
| `crates/proto` | `calendry-solver-proto` | `build.rs` codegen only, no hand-written logic | core |
| `crates/service` | `calendry-solver` | tonic server, run registry, **and the proto↔core conversion** | — |
| `crates/gen` | `calendry-solver-gen` | Benchmark generator and `bench` harness | the service |

`crates/service` has both a library and a binary target, so everything in it has
a test surface ([ADR-0018](docs/adr/0018-the-service-crate-has-a-library-target.md)).
Conversion is not its own crate yet; [ADR-0017](docs/adr/0017-conversion-errors-are-typed-transport-mapping-is-one-place.md)
records what would make the split mechanical.

Correctness fixtures are hand-written in `crates/core/src/testing.rs`, grouped by
the behaviour they exercise, and kept separate from the generator on purpose
([ADR-0009](docs/adr/0009-generator-separate-from-correctness-fixtures.md)).

---

## How to run

```bash
git clone --recurse-submodules …     # or: git submodule update --init --recursive

cargo test --workspace               # 183 tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check

CALENDRY_SOLVER_ADDR=127.0.0.1:50051 cargo run -p calendry-solver
```

CI runs all of the above plus rustdoc and a release benchmark smoke run
([ADR-0020](docs/adr/0020-workspace-lints-and-ci-are-the-gate.md)). Lints live in
the root `Cargo.toml` and are inherited by every crate.

Every job that compiles goes through `.github/actions/rust-build-env`, which owns
the pinned submodule, **`protoc`**, the toolchain and the cache. protoc is the one
that bites: it is a build dependency of `crates/proto` that no `Cargo.toml`
declares, so a workspace is green on any machine that happens to have it and
fails in CI on the first run. Add compiling jobs through that action, not by
hand.

There is a second workflow, `.github/workflows/docker.yml`, building the image
from the root `Dockerfile`. It is independent of the gate above — a red test does
not stop an image build and vice versa.

### Benchmarks

Release only — a debug build measures the drift assertion rather than the search —
and move budgets only, because a wall-clock-terminated run is not reproducible.

```bash
cargo run --release -p calendry-solver-gen --bin bench -- \
    [preset...] [--gen-seed N] [--seeds N] [--moves N] [--wall S] \
    [--calibrate] [--diagnose N] [--evaluate] [--elective RATIO]
```

### Poking the service by hand

Example payloads live in `examples/`. The service does **not** expose gRPC
reflection, so `grpcurl` needs the proto files explicitly:

```bash
grpcurl -plaintext -import-path vendor/calendry-proto/proto \
  -proto calendry/solver/v1/service.proto \
  -d @ 127.0.0.1:50051 calendry.solver.v1.SolverService/StartRun \
  < examples/forced_unique.json
```

Enum values are prefixed on the wire, so JSON callers send
`"WEEK_KIND_TEACHING"`, not `"TEACHING"` — see [`docs/SCHEMA.md`](docs/SCHEMA.md).

### Updating the schema pin

A deliberate act, producing a one-line reviewable diff. `--remote` must never
appear in a build script or in CI.

```bash
git submodule update --remote vendor/calendry-proto
cd vendor/calendry-proto && git checkout v0.3.0   # a TAG, not a branch tip
cd ../.. && git add vendor/calendry-proto && git commit -m "proto: bump to v0.3.0"
```

---

## What is built, and what is not

**Every catalogue constraint type the schema defines is implemented, with one
exception.** `PersonPreferenceFit` arrived in schema v0.7.0 and is refused as
`UNIMPLEMENTED`; everything else is evaluated. The conversion layer's match is
exhaustive with no `_ =>` arm, so a new type in the schema is a compile error
rather than a silently ignored setting — which is the property that mattered.

The catalogue is no longer exactly fourteen types, and counting them is not a
useful check: `MinimizeBlockUsage` replaced two types with one carrying flags
([ADR-0024](docs/adr/0024-one-type-per-axis-with-flags.md)), and the two it
replaced remain on the wire as deprecated.

Search is greedy construction followed by Large Neighborhood Search with
simulated-annealing acceptance, driving `MoveEvaluator` for real. All four
benchmark presets place every Session; a 27,136-Session university solves in
~250 ms. **Performance work is closed** —
[ADR-0021](docs/adr/0021-measure-end-to-end-before-optimizing-a-component.md)
explains why, and what not to reopen without a new measurement.

The one open question about search *quality* rather than speed is that nothing
currently bounds online usage until LNS, and LNS barely runs at the default move
budget. That is measured, not suspected —
[ADR-0025](docs/adr/0025-maxonlineshare-is-not-enforced-by-the-search.md), and
read it before changing a preset.

Deliberately not built:

* **v2 minimize-movement lock policy.** `LOCK_POLICY_MINIMIZE_MOVEMENT` returns
  `UNIMPLEMENTED`. [ADR-0008](docs/adr/0008-one-solve-mechanism-scope-plus-lock-policy.md)
  covers the shape; `Immovable` already records *why* each Session is immovable
  so that v2 is a policy change rather than a rewrite.
* **A GPU move-evaluation backend.** The seam exists and has two adapters; the
  backend does not. [ADR-0013](docs/adr/0013-move-evaluation-behind-a-trait.md).

Outside this repo: the Nuxt integration session, including the one part of the
schema pipeline never exercised end to end. See
[`docs/SCHEMA.md`](docs/SCHEMA.md).

---

## Three constraint shapes — read before adding a type

1. **Pairwise, keyed by `(entity, slot)`** — the four structural double-booking
   types. Occupancy bitsets; the search can never violate them. Note that the
   room axis exempts non-exclusive Rooms
   ([ADR-0022](docs/adr/0022-a-virtual-room-is-not-an-exclusive-resource.md)).
2. **Unary, keyed by `(slot, room)`** — the soft types, and also `LecturerVeto`,
   which despite its name depends only on one Session's slot and its lecturers.
   Precomputed lookup tables and masks; O(1) exact deltas.
3. **Aggregate over a set** — `OnlineOnsiteSameDay` and `MaxOnlineShare`, in
   `aggregates.rs`. Neither is expressible as a slot-keyed bitset, and **neither
   is a filter any more**.

Within shape 3 the two types still differ, and the difference is load-bearing —
it is what "hard" and "soft" reduce to once neither can be a filter:

* **`MaxOnlineShare` cannot be a filter at all.** It is a cardinality ratio,
  invisible in any pair, and a filter would dead-end construction because the
  first online Session placed makes the ratio 100% before the denominator has
  grown. Under `PER_WEEK` the denominator also *moves* when a Session relocates
  between weeks. So it lives on the objective, **charged at `hard_penalty`**. A
  run can therefore succeed while still reporting a `MaxOnlineShare` violation —
  the same shape as `ExactFrequency` reporting unplaced Sessions, not a new
  exception.
* **`OnlineOnsiteSameDay` used to be a filter and is now priced**, at its
  configured weight rather than at `hard_penalty`
  ([ADR-0023](docs/adr/0023-onlineonsitesameday-is-priced-not-forbidden.md)). That
  weight difference is the entire distinction between the two. Because the search
  now produces mixed days on purpose when the alternative costs more, a mixed day
  is **not** a hard violation: it is carried in the objective breakdown with its
  cell count and weighted cost, where every other soft type's breaches are.

Neither term can be attributed to a single placement — a violated share cell and
a mixed `(group, day)` cell each belong to a set — so `Trial` reads both straight
off the counters instead of accumulating them as deltas, and `ruin_worst` is blind
to both ([ADR-0025](docs/adr/0025-maxonlineshare-is-not-enforced-by-the-search.md)).

## Group scoping differs by constraint, deliberately

* **Double-booking** propagates **both** directions — ancestors and descendants.
* **Attendance, and both Group-scoped aggregate types**, propagate **downward
  only**. A cohort Session implicates its classes' members; a class Session does
  not implicate the cohort.

## Other things worth knowing before changing this code

* **The objective is maintained incrementally, and one module owns it.** `Trial`
  holds the solution, the incremental index and the objective together, so they
  cannot disagree; a rejected round is reversed from a journal in O(k). Debug
  builds assert on every iteration that the maintained objective matches a
  from-scratch recomputation. The share counters have a *moving denominator*,
  which is more error-prone than the soft sums, so the aggregate-drift test is
  the highest-value test in the suite.
* **Search hyperparameters are not domain magic numbers.** `search::tuning` holds
  the cooling rate, stagnation limit and candidate cap. The ban is on *domain*
  assumptions — `slot % 3`, `timeslot > 14`, `weeks[-n:]`.
* **Construction is seed-independent**; the seed influences only the LNS phase.
* **Negative soft weights and out-of-range share ratios are rejected**, as is a
  `MaxOnlineShare` with no window — a ratio is meaningless without one.
* **Two search defects were found by falsification tests**, and both are worth
  knowing because the fix is easy to undo by accident: LNS never retried
  Sessions construction left unplaced (so the `unplaced` term was permanently
  unoptimizable), and repair broke ties by lowest index (which collapsed the
  neighbourhood — ruining the same Session always regenerated the same
  placement). Ties are now broken with the seeded RNG: still reproducible, but
  the neighbourhood is real.

---

## Reference

Prototype: **TimeCraft**, a prior student project — Python, CP-SAT via OR-Tools.
Its constraint set is the origin of this catalogue. Its hardcoded assumptions
(`timeslot % 3`, `timeslot > 14`, `weeks[-exam_weeks:]`, a 30% online cap) are
exactly what the parametrized versions replace — treat any resemblance to those
magic numbers in new code as a bug.
