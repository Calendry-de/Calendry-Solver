# calendry-solver

Rust optimization service for **Calendry**, a multi-tenant timetabling platform.
This service does the scheduling math; the Nuxt app owns everything else.

> **This file is the authoritative context for this repo.** It was written by
> distilling `TAXONOMY.md` (the platform-wide entity/architecture record, which
> lived here temporarily and has since been deleted) plus a separate solver
> planning process. Nothing below can be re-derived from code or git history —
> if a fact here conflicts with an assumption you're about to make, this file wins.

---

## 1. Domain model (fixed — changing any of it is a migration, not config)

Calendry deliberately separates a **fixed core taxonomy** (entity types and
their relationships) from an **open, tenant-configurable vocabulary** (the
*values* filling those entities: role names, equipment tags, session kinds,
constraint parameters). Tenants extend the vocabulary freely; they never
touch the schema.

### Organizational hierarchy
- **Federation** — optional parent grouping of tenants that share resources
  (a university consortium sharing a lecture hall or a cross-enrolled elective).
  Owns resources that member tenants may reference.
- **Tenant** — a single institution. Fully data-isolated *except* for
  explicitly Federation-owned resources. Multi-tenant from day one.

### People, roles, grouping
- **Person** — the only person entity. No separate Student/Lecturer/Staff tables.
- **Role** — tenant-defined vocabulary attached to a Person. `Lecturer` is the
  one fixed, universal role name (the person leading a Session). Everything else
  (Student, Auditor, TA, External Participant, …) is tenant-defined.
- **Group** — class, cohort, seminar group. **Nested** (parent/child hierarchy:
  Cohort → Class → Seminar Group).
- **Membership** — Person ↔ Group relation.
- **Conflict propagation rule**: a scheduling conflict on a parent Group
  propagates to block its child Groups, and vice versa. "Is Group G free at
  time T" therefore requires checking G's full **ancestor *and* descendant**
  chain, never a flat lookup.

### Space
- **Room** — capacity, a ranking/desirability value, location.
- **Equipment / Feature** — tenant-defined tags on a Room (projector, PC lab,
  lab bench), referenced by Offerings that require them.
- **Online delivery is a virtual Room**, not a boolean flag — keeps
  room-assignment logic uniform. (Some soft constraints still reason about
  "is this session online", so the solver must be able to identify virtual rooms.)

### Scheduling — the two-level demand-vs-instance split
This is the single most important structural fact about the model.

- **Offering** — the **demand** definition: "this needs to happen N times, needs
  a Lecturer with role X, a Group, a Room with equipment Y, kind Z."
  *This is the solver's input.*
- **Session** — one **atomic, placed instance**: a specific week / timeslot /
  room / lecturer(s) / group(s). This is what gets displayed, moved, swapped,
  locked, exported, and notified about. *Solver output (and manual edits)
  operate at this level.*
- **kind** — tenant-defined vocabulary on Offering/Session, replacing any fixed
  Lecture/Exam/Event split. **Every constraint type must declare which kinds it
  applies to**, because a tenant-defined kind (e.g. `staff_meeting`) may have no
  Group at all.
- **Assignment** — relations: Session ↔ Group, Session ↔ Person (direct
  individual), Session ↔ Room, Session ↔ Lecturer.

### Time
- **TimeGrid** — a per-tenant configured entity: block length, blocks/day,
  active days, start hour. **There is no global fixed grid.** Any arithmetic like
  "slot % 3 = the day" or "slot > 14 means Saturday" is forbidden — everything
  resolves against the requesting tenant's TimeGrid.
- **Academic calendar** — core from day one: Terms/Semesters, Holidays, Break
  weeks, Exam periods, as structured data. Constraints like "minimize sessions in
  exam weeks" resolve against this, never against `weeks[-n:]`.
- All solving and grid logic happen in **single institution-local time**.
  Per-Person timezone is a presentation/export concern only and **must not leak**
  into "same day" / "adjacent slot" constraint logic.

### Federation isolation
`tenant_id` and `federation_id` are both nullable with a CHECK that **exactly
one is set**. Three entities are Federation-shareable:

- **Room** — a shared lecture hall.
- **Offering** — a cross-enrolled elective.
- **Session** — a genuinely shared event spanning tenants (e.g. a
  university-wide celebration where Technology and Medicine are separate tenants
  under one Federation). This is *one* event, not a coincidence of two identical
  per-tenant events.

Session being shareable was a later amendment; the app-side relation tables
(`session_group`, `session_person`, …) and their RLS were designed against a
two-table exception and still need extending. That is **app-repo work, not
solver work** — noted here only so the solver never assumes Session is always
tenant-scoped.

### Editing & history (app-owned, but shapes solver input)
- Solver runs produce an immutable versioned **Generation** (snapshot); manual
  edits are an append-only event log (`create`, `move`, `swap`, `delete`, `lock`)
  applied on top of it.
- **A locked Session is never overwritten by the solver.**
- Manual-edit conflict UX is **warn and allow** — the app may hand the solver a
  starting state that already violates hard constraints. The solver must not
  assume its input is feasible.

---

## 2. Solver architecture decisions

Decided in a separate planning process; **not derivable from the taxonomy**.

### Problem class
A **Constraint Optimization Problem**: hard constraints define feasibility, soft
constraints define a weighted objective.

Approach: **hybrid constructive heuristic + local search** (simulated annealing /
Large Neighborhood Search). **CPU-only for v1**, using `rayon` for data
parallelism. The **move evaluator sits behind a trait/interface** so a future GPU
backend could plug in specifically for LNS candidate-move scoring. That backend
is *not being built now* — the point is only that the architecture must not
foreclose it.

### Data flow
**The solver never touches Postgres.** Nuxt assembles a `SolverInput` snapshot
and sends it over gRPC. The solver is stateless and input/output-only.

### Schema / transport
- Protobuf definitions live in a **separate repo**:
  `https://github.com/MindCollaps/calendry-proto`, consumed as a **pinned git
  dependency** with `prost` codegen. **Do not copy `.proto` files into this repo.**
- Transport is **gRPC, unary calls only** — no held-open streams (see the job
  model below for why).

### Solve mechanism — one mechanism, not multiple modes
A solve request carries:
- a **scope** — which Offerings/Groups are being actively placed, and
- a **lock policy** for everything outside that scope.

**v1**: everything outside scope is **hard-locked** and never moved.
**v2 (deferred, explicitly not this step)**: replace the hard lock with a soft
*minimize-movement* constraint, so the solver *may* disturb out-of-scope
Sessions when genuinely necessary to resolve a conflict, but is heavily biased
against doing so.

**In both versions: Sessions whose time has already passed are excluded from
recalculation entirely.** This is a correctness rule, not a tunable preference.

### Job model
The solver **owns run state** and exposes three unary gRPC calls:

| Call | Behavior |
|---|---|
| `StartRun(SolverInput, scope, budget)` | Returns a run id **immediately**, begins optimizing in the background |
| `GetStatus(run_id)` | Returns status / progress / best-objective-so-far |
| `CancelRun(run_id)` | Stops an in-progress run |

Nuxt **polls `GetStatus`** and persists progress into its own `solver_run` table.
**This repo does not persist run state beyond an in-progress run's lifetime** —
no database, no on-disk run journal. Run state dies with the process.

### Termination
**Both** a time budget **and** an iteration/move-count budget; **whichever hits
first** ends the run. Both configurable **per request**.

#### Determinism has one inherent limit — read before writing a test
Same seed gives byte-identical output **only when termination is deterministic**:
`"converged"` or `"move_budget"`. A run stopped by the **wall-clock budget**
cannot be reproducible, because the number of LNS iterations completed depends on
machine speed and load. That is inherent to a time-boxed metaheuristic, not a
defect, and it is why `termination_reason` exists — a caller can tell which
guarantee they got.

**Tests must therefore use move budgets, never wall-clock budgets.** A
determinism test written against `max_wall_millis` will look flaky and will
waste somebody's afternoon.

### Constraint catalogue — 14 predefined types
Each type gets **one typed, compiled evaluator function** reading that type's
typed parameters. There is **no interpreter and no free-form expression DSL** —
**tenant-supplied logic never executes**. Constraints are configured by choosing
a predefined type and filling in typed parameters.

Each type declares: hard vs. soft, which `kind`(s) it applies to, its parameters,
and (if soft) a penalty weight. **Hard-vs-soft is a property of the type**, not a
per-tenant config field — it is compiled into the evaluator.

**4 structural types** (all hard, all double-booking):
1. Room double-booking (room + timeslot + week)
2. Lecturer double-booking
3. Group double-booking — **including nested-group ancestor/descendant propagation**
4. **Person double-booking** — catches a clash the Group check structurally
   cannot: a Person who is a member of two Groups **unrelated in the nesting
   tree** (neither an ancestor nor descendant of the other), both scheduled at
   the same slot. Considers direct individual assignment *and* Group membership.

Types 1–3 are **already evaluated by the Nuxt app today**. Type 4 is **not** —
see the cross-repo follow-up below.

**10 solver-owned types** — currently configurable in the Calendry UI but
**visibly inert until this service implements them**:

*Hard:*
5. Exact frequency per Offering (N placed Sessions per Offering)
6. Lecturer vetoes (day/slot blackout) — the blackout **values** live on
   `Person.blackouts` (per-person data); the constraint config is tenant-level
   policy that merely switches enforcement on
7. Online + on-site same-day exclusion, per Group
8. Max % online per Group *(was hardcoded 30% in the prototype — now a
   `max_ratio` parameter, with a `PER_TERM`/`PER_WEEK` window, both kept
   configurable)*

*Soft:*
9. Minimize first-block usage
10. Minimize last-block usage *(generalizes the prototype's "third block" —
    with per-tenant `blocks_per_day`, the last block is not fixed at index 2)*
11. Minimize **day** usage, parametrized by ISO weekday *(generalizes the
    prototype's hardcoded "minimize Saturday" / `timeslot > 14`; with
    tenant-configured `active_days`, Saturday is not structurally special)*
12. Minimize high-ranking room usage *(`Room.rank` is ordered **higher =
    premium/scarce**; rooms at or above a `rank_threshold` are penalized)*
13. Minimize sessions in exam weeks *(**must** resolve against the Academic
    Calendar, not `weeks[-n:]`)*
14. Minimize online sessions

### TRACKED CROSS-REPO FOLLOW-UP — PersonDoubleBooking in the Nuxt app
**This is app-repo work for a future session, not solver work.** Recorded here
because this repo is where the decision was made.

The Nuxt app's manual-edit constraint evaluator (built in the app's Steps 4–5)
currently checks room, lecturer, and group double-booking, but **not person
double-booking**. Without it, a manual edit can create exactly the clash type 4
describes — a student in two tree-unrelated Groups scheduled at the same slot —
**and the user is never warned**, because the "warn and allow" UX can only warn
about violations its evaluator knows how to detect.

The solver implementing type 4 does *not* fix this: the solver only sees a
snapshot at solve time, whereas manual edits happen continuously between runs.
Both evaluators need the check, and they must agree on its semantics.

### Nested-group performance requirement
The parent/child conflict propagation is evaluated in the local-search hot loop,
potentially millions of times per run. It must use **precomputed in-memory
ancestor/descendant sets**, never a live tree walk. (The app side separately uses
a closure table / recursive CTE for the same rule — same semantics, different
representation, and they must agree.)

### OPEN DEPENDENCY — federation-shared resource occupancy
A tenant's `SolverInput` snapshot needs **occupancy visibility into
Federation-shared Rooms used by *other* tenants**, so the solver doesn't
double-book a shared resource across a tenant boundary.

**The mechanism is undecided on the app side** — candidates were a cross-tenant
occupancy ledger versus a narrow database function. **Do not assume a shape for
this and do not design around a guess.** Treat it as an open dependency, and
raise it rather than resolving it unilaterally. The solver-side requirement is
only that the input format can carry opaque external-occupancy blocks against
shared Rooms.

### Benchmark data
A **parametrized generator** spanning small-school through large-university
scale, with a few **named presets** built on top, for performance and solution-
quality testing. **Kept separate from correctness test fixtures** — the two have
different purposes and should not share a source of truth.

#### Room tightness is NOT how hard an instance is — read before touching a preset
The obvious difficulty measure is `demand_blocks / (rooms x slots)`. It is the
wrong one, and calibrating against it produces instances construction cannot
solve at all (measured: 565 of 3968 Sessions placed).

The reason is conflict propagation. A Room's row accumulates only the Sessions
placed in that Room. A **Group's** row accumulates every Session of every Group
in its conflict closure, so a Cohort is marked busy by its entire subtree.
Demand that spreads across many Rooms piles onto a *single* Cohort row. The
generator therefore calibrates the **binding axis**:

```
saturation = max( demand / (rooms x slots),         // room
                  max_g blocked_blocks(g) / slots,  // group  <- always binds
                  max_l taught_blocks(l) / slots )  // lecturer
```

Target band 0.55..=0.75. At realistic hierarchies the group axis binds in every
preset and room tightness sits at 0.24–0.38 — that is correct, not a defect.

Cohorts are assigned **round-robin, not at random**. Because the cohort row
binds, random assignment lets the *busiest* cohort decide feasibility, and the
max of N draws grows with N — measured saturation overshot the closed form by
1.28x at school scale and 1.55x at university scale, so no single calibration
held across the range. Class and seminar choice within a cohort stays random.

#### A load metric CANNOT certify feasibility — the person-clique bound
Group, lecturer and room load all ask *"how busy is one row"*. There is a whole
class of infeasibility they structurally cannot see, and slice 5 shipped four
presets that were **provably unplaceable** while every axis read "in band".

Two Offerings sharing even one attendee can never occupy the same slot under
`PersonDoubleBooking`. A set that **pairwise** conflicts therefore needs one
distinct slot per Session. Every individual can be lightly loaded while the
attendee *sets* pairwise intersect — that is a graph-colouring bound, not a load
bound.

So `InstanceStats` carries `person_clique_load`: a greedy maximum-clique lower
bound over the Offering conflict graph, times its Sessions' block demand, over
the term. **Above 1.0 is a certificate of infeasibility.** Greedy understates the
true clique, so it can miss infeasibility but never invent it — below 1.0 is
*not* a proof of feasibility. It is part of `saturation`, and
`presets_are_calibrated_into_the_saturation_band` asserts it stays under 1.0.

#### Electives are their own root-level Groups — never another Cohort's seminar
The original model enrolled a student into a Seminar belonging to a *different*
Cohort. That put them in the other Cohort's subtree, which made them an attendee
of its entire cohort-wide lecture series. One shared student then made two
Cohorts' lectures mutually exclusive, and with 30% cross-enrolment across 80
cohorts **94.8% of lecture pairs conflicted** — 1,146 Sessions needing at most
350 slots. Construction left 2,468 of 25,520 unplaced and no solver could have
done better.

Electives are now **root-level Groups with their own Offerings**, which is what
an elective actually is. They stay tree-unrelated to the student's home Seminar,
so `PersonDoubleBooking` still has work the Group check cannot do, without
welding two Cohorts together. Elective groups are **Class-sized**: seminar-sized
produced 360 groups at large-university whose Offerings added 56% to total
demand — an elective programme larger than the core curriculum.

---

## 3. Non-negotiables checklist

Before writing solver code, check the change against these:

- [ ] No Postgres, no DB driver, no persistence beyond in-flight run state.
- [ ] No `.proto` files in this repo — consume `calendry-proto` as a pinned git dep.
- [ ] No timeslot arithmetic that hardcodes day/week structure — resolve against TimeGrid.
- [ ] No exam-week or holiday logic by array slicing — resolve against the Academic Calendar.
- [ ] No per-person timezone anywhere in grid or constraint logic.
- [ ] No expression evaluation of tenant-supplied strings — typed parameters only.
- [ ] Past Sessions excluded from recalculation, always.
- [ ] Locked / out-of-scope Sessions never moved (v1: hard lock).
- [ ] Group conflict checks use precomputed ancestor+descendant sets.
- [ ] Move evaluation stays behind the trait boundary (future GPU backend).
- [ ] Solver tolerates infeasible input — the app's "warn and allow" UX can produce it.

---

## 4. Repository layout & how to run

Four crates. The split exists because **core must not know prost types exist** —
generated protobuf types are `String`-id'd, `Option`-wrapped and heap-heavy, and
a local search evaluating millions of moves cannot touch that representation.
Separate crates make erosion of that boundary a compile error.

| Path | Crate | Role | Must not depend on |
|---|---|---|---|
| `crates/core` | `calendry-solver-core` | Domain model, dense indices, slot tables, evaluators, search | prost, tonic, tokio, I/O, any clock |
| `crates/proto` | `calendry-solver-proto` | `build.rs` codegen only, no hand-written logic | core |
| `crates/service` | `calendry-solver` (bin) | tonic server, run registry, **and the proto↔core conversion module** | — |
| `crates/gen` | `calendry-solver-gen` | Benchmark generator + `bench` harness | the service |

Conversion is deliberately *not* its own crate yet; promote it when its
validation logic grows. Correctness fixtures are hand-written in
`core/src/testing.rs`, kept separate from the generator on purpose.

### Locating calendry-proto — git submodule at `vendor/calendry-proto`
`.proto` files are **never vendored here**. The schema is consumed as a **git
submodule pinned to an exact revision**:

```
vendor/calendry-proto/          <- submodule, url = github.com/MindCollaps/calendry-proto
    proto/calendry/solver/v1/   <- the include root build.rs compiles against
```

`crates/proto/build.rs` resolves the checkout in this order:
1. `CALENDRY_PROTO_DIR` env var — explicit override for CI / reproducible builds.
2. `vendor/calendry-proto/proto` — the submodule.
3. **Nothing.** A missing submodule is a hard error quoting
   `git submodule update --init --recursive`.

**There is deliberately no sibling-checkout fallback.** An earlier revision fell
back to `../calendry-proto/proto`; it was removed because a sibling checkout is
unpinned and unversioned, so the fallback let the build succeed against whatever
happened to sit in a neighbouring directory whenever the submodule was missing.
That is exactly the silent schema-drift failure this repo guards against
elsewhere. Do not reintroduce it.

**Pinning policy.** The submodule records an exact SHA in this repo's tree, so
pinning is automatic; the discipline is in how it *moves*. `branch = main` in
`.gitmodules` affects **only** `git submodule update --remote` — it is not
auto-tracking, and `--remote` must never appear in a build script or in CI.
Updating the schema is a deliberate act:

```bash
git submodule update --remote vendor/calendry-proto
cd vendor/calendry-proto && git checkout v0.2.0   # a TAG, not a branch tip
cd ../.. && git add vendor/calendry-proto && git commit -m "proto: bump to v0.2.0"
```

The diff is one reviewable line (`-Subproject commit …` / `+Subproject commit …`).
Schema movement cannot enter this repo without a commit that says so.

**Pin to tagged commits, not loose SHAs.** The tag is what ties the two consumers
together: this repo pinned at the commit tagged `v0.1.0`, and the Nuxt app
installing `@mindcollaps/calendry-proto@0.1.0` built from that same tag, means
"which schema is each side on" has one answer. A loose SHA works mechanically but
breaks that correspondence.

```bash
git clone --recurse-submodules …   # or: git submodule update --init --recursive
cargo test --workspace             # 96 tests
cargo clippy --workspace --all-targets
CALENDRY_SOLVER_ADDR=127.0.0.1:50051 cargo run -p calendry-solver

# Benchmarks. Release only — a debug build measures the drift assertion, and
# move budgets only, because a wall-clock-terminated run is not reproducible.
cargo run --release -p calendry-solver-gen --bin bench -- \
    [preset...] [--gen-seed N] [--seeds N] [--moves N] [--wall S] [--calibrate]
```

Example request payloads live in `examples/`. The service does **not** expose
gRPC reflection, so `grpcurl` needs the proto files explicitly:

```bash
grpcurl -plaintext -import-path vendor/calendry-proto/proto \
  -proto calendry/solver/v1/service.proto \
  -d @ 127.0.0.1:50051 calendry.solver.v1.SolverService/StartRun \
  < examples/forced_unique.json
```

### Schema distribution — DONE AND VERIFIED (2026-08-15)
The contract repo, this repo's consumption of it, and its CI/CD are all real and
observed working. Only the Nuxt side remains, and it is a separate session.

**calendry-proto — schema, tag, and registry**
- Three `.proto` files at `7856748`, tagged **`v0.2.0`** (annotated `a76a203`),
  pushed to `github.com/MindCollaps/calendry-proto`.
- **`@mindcollaps/calendry-proto@0.2.0` is published** to GitHub Packages and
  publicly listed. Verified via the public package page; the registry REST and
  npm endpoints return 401/403 unauthenticated, so *installability* was not
  exercised here — that is the Nuxt session's first task.
- `v0.1.0` still exists as a historical marker but was **never published**: the
  enum rename below superseded it before it ever shipped.

**Enum values are prefixed** — `WEEK_KIND_*`, `SHARE_WINDOW_*`, `RUN_STATUS_*`.
protobuf scopes enum values to the enclosing *package*, not to their enum, so
bare `EXAM`/`BREAK`/`RUNNING` would collide with any future enum wanting the same
word — a protoc error, not a warning. Consequences, measured not assumed:
- **Rust: unchanged.** prost strips the enum-name prefix, so variants stay
  `Teaching`, `Exam`, … and no solver source needed editing.
- **Binary wire: unchanged.** Only names moved, never numbers.
- **Proto3 JSON: changed**, since JSON uses the value name. Callers must send
  `"WEEK_KIND_TEACHING"`, not `"TEACHING"` — the old value is now rejected
  outright (`enum "calendry.solver.v1.WeekKind" does not have value named
  "TEACHING"`). `examples/*.json` were updated accordingly.

**CI/CD in calendry-proto, proven in both directions**
- `buf.yaml`: `STANDARD` + **`UNARY_RPC`** lint, `FILE` breaking ruleset.
  `UNARY_RPC` mechanically enforces the unary-only architecture — verified it
  rejects a `stream` RPC.
- `validate.yml` on every branch push and PRs to main: `buf lint`,
  `buf breaking` vs main, and an independent `protoc` compile. The breaking check
  is **skipped on main by design** (there it would compare main against itself);
  the gate that matters runs on branches and PRs.
- The protoc step carries `if: always()` so a `buf breaking` failure cannot mask
  a protoc failure.
- `publish.yml` on `v*.*.*` tags + `workflow_dispatch` with `dry_run` defaulting
  to **true**. The git tag is the sole source of truth for the version; a
  dispatch from a branch requires an explicit `version` input and otherwise fails
  fast rather than publishing garbage.
- **Verified by observation, not by reading the YAML:** a throwaway branch
  renumbering `Room.rank` 6→99 produced a red run failing exactly at
  `buf breaking` (`Previously present field "6" ... was deleted.`, exit 100);
  reverting produced a green run where breaking *actually compared* rather than
  skipping; and a combined break+fix run showed the protoc step flip from
  `skipped` to `success`, proving `if: always()`.

**This repo's pin**
- Submodule `vendor/calendry-proto` pinned to `7856748` = **`v0.2.0`**.
- All 26 tests pass against the renamed schema, clippy clean, and the gRPC path
  was re-checked end to end with `grpcurl`.

**Known gap, deliberately accepted:** GitHub Actions job logs need an
authenticated token, so CI's own tarball listing could not be read. It was
verified by reproducing the identical build from the `v0.2.0` tag
(17 files, 40.4 kB packed / 391.2 kB unpacked; only `dist/**` + `package.json`,
no `src/generated`, no `node_modules`, no `.proto`).

### STILL TO DO — the Nuxt (`calendry`) repo, separate session
That repo is **not checked out here**; do not attempt this from the solver repo.
- Add `calendry-proto` as a submodule there too, pinned to the same `v0.2.0`.
- Install and import `@mindcollaps/calendry-proto@0.2.0`, and wire the gRPC
  client.
- **Add an `.npmrc` with a GitHub Packages token.** The registry requires
  authentication even to *install* a public package — this hits local dev, the
  docker-compose build, and CI. `calendry` has no `.npmrc` today. This is the
  one part of the pipeline never exercised end to end.
- Add `PersonDoubleBooking` to the app's manual-edit evaluator (see the separate
  follow-up above).

The tag is what keeps the two consumers honest: this repo pinned at the commit
tagged `v0.2.0`, and the Nuxt app installing `@mindcollaps/calendry-proto@0.2.0`
built from that same tag, means "which schema is each side on" has one answer.

### Implementation status
**Slices 1-5 complete. All 14 catalogue types are implemented.** There is no
longer an `UNIMPLEMENTED` branch in `convert.rs` — a new type added to the schema
fails to compile against the match instead, which is the property that mattered.

Search is greedy construction followed by Large Neighborhood Search with
simulated-annealing acceptance, driving `MoveEvaluator` for real.

Remaining: the two scaling defects slice 5 measured (below), and the deferred v2
minimize-movement lock policy.

#### MEASURED — where a run's time actually goes
Numbers from `cargo run --release -p calendry-solver-gen --bin bench`, taken
against **corrected** instances (slice 6a). The slice-5 figures were measured on
infeasible instances and are superseded.

All four presets now place **every** Session, and LNS runs properly at every
scale:

| preset | placements | construct | evaluate_hard | LNS | iterations | improvements |
|---|---|---|---|---|---|---|
| small-school | 1,497 | 31 ms (44%) | 1.1 ms (2%) | 38 ms (54%) | 91 | 82 |
| large-school | 3,167 | 108 ms (70%) | 5.8 ms (4%) | 40 ms (26%) | 84 | 76 |
| small-university | 6,760 | 560 ms (81%) | 46 ms (7%) | 83 ms (12%) | 89 | 77 |
| large-university | 27,136 | **7.12 s (91%)** | 625 ms (8%) | 47 ms (1%) | 80 | 67 |

**Construction is the scaling problem, and it is the only one left.** First-fit
scans `slots x eligible_rooms` per placement, and the per-candidate cost is
constant across a 15x range of candidate-space size — 3.67 / 3.24 / 3.18 / 3.11
ns — which is the signature of scanning essentially the whole space every time.

`evaluate_hard` is now second at 8%; it is the end-of-run violation report, once
per run.

**H2 is gone, at every preset size.** It was a symptom of infeasibility, not an
independent defect: with 0 unplaced there is nothing to retry, and LNS completes
80-91 iterations everywhere. **H1 is fixed but was never the lever** — repair
sits inside a 1% slice at large-university, so its 148x enumeration waste was
worth well under 1% of wall time.

#### 60% of construction's inner loop is provably wasted
Measured by the `--diagnose` mode, consistently across all four presets:

| preset | start slots rejected by a room-INDEPENDENT axis | probes wasted |
|---|---|---|
| small-school | 58.9% | 58.4% |
| large-school | 59.0% | 58.4% |
| small-university | 62.7% | 61.9% |
| large-university | 60.2% | 59.7% |

Of the six axes only **room occupancy** and **day-mix** (via virtual-vs-physical)
depend on which Room is being tried. Lecturer, group, person and veto do not —
yet `construct` re-tests them once per eligible Room at every slot. At a slot
already rejected by one of them, the entire room loop cannot succeed.

The waste is worse than the probe count suggests: the room check is one bit test
and exits early, while the room-*independent* path scans the attendee list (mean
65, max ~300). So the expensive scan currently runs once per *free* Room per
slot instead of once per slot.

#### FIXED — locked Sessions used to be double-counted
Found by slice 5, fixed standalone straight after it. `Session.offering_id`
existed on the wire but `convert.rs` dropped it when building a `FixedSpec`, then
created placement variables for the **full** `required_session_count` of every
in-scope Offering. An Offering requiring 12 Sessions with 3 already locked got 12
new placements on top of the 3 — **15 total**. That is the ordinary mid-term
re-solve, the exact case the lock policy exists for.

Both halves were the same gap — the Offering↔locked-Session link was lost at the
boundary — so both halves were closed:

- `FixedSpec` / `FixedOccupancy` carry `offering: Option<OfferingIdx>`. `None` is
  correct and meaningful: ad-hoc Sessions (a `staff_meeting` kind) realize no
  Offering, and external Federation occupancy belongs to another tenant.
- `constraints::exact_frequency` counts immovable Sessions toward their
  Offering. A locked or past Session is still a Session that happened.
- `convert.rs` places `required_session_count.saturating_sub(already_realized)`.
  **`saturating_sub`, never `-`**: "warn and allow" means a caller can send more
  Sessions than an Offering claims to need, and wrapping a `u32` would ask the
  solver to place four billion Sessions.

Nothing else reads `problem.fixed` beyond span/room/lecturers/groups/attendees,
so `Occupancy::from_fixed`, `SearchState::from_fixed` and `collect_views` were
untouched, and there was **no schema change** — the field already existed on the
wire.

It left one gap open, tracked immediately below.

#### KNOWN GAP — over-supplied Offerings report no violation
**Over-supplied Offerings — locked Sessions exceeding `required_session_count` —
currently report no violation.** An Offering requiring 2 with 4 locked Sessions
against it passes silently.

The cause is that `constraints::exact_frequency` uses **placement-variable
presence as its proxy for "in scope"**, and the locks-deduction added by the fix
above can now drive that count to zero. An over-supplied Offering therefore looks
identical to an out-of-scope one, and is skipped.

**Fixing it requires carrying real scope membership into `Problem` — a core
semantic change, not a boundary fix.** That is why it was not bundled into the
double-counting fix, which stayed entirely at the conversion layer.

What *is* guaranteed: the deduction uses `saturating_sub`, so over-supply yields
zero placements rather than a `u32` underflow. A test in `convert.rs`
(`more_locks_than_required_saturates_instead_of_underflowing`) asserts today's
behaviour, so changing it has to be deliberate — but the decision lives here, not
only in that test's comments.

#### BREAKING CHANGE for the Nuxt integration: `GetStatus.best_objective`
Through slices 1-2 this field carried the **hard-violation count**. Since slice 3
it carries the **real weighted objective** (`hard x hard_penalty + soft_sum`).
Nothing consumes it yet, but anything built against the old meaning must be
updated. `ObjectiveBreakdown` is also populated; it previously shipped empty.

#### The three constraint SHAPES — read before adding a type
Slices 1-3 needed two shapes. Slice 4 needed a third, and forcing the new types
into the old ones would have been the mistake:

1. **Pairwise, keyed by `(entity, slot)`** — the four structural double-booking
   types. Occupancy bitsets; the search can never violate them.
2. **Unary, keyed by `(slot, room)`** — the six soft types, and also
   `LecturerVeto`, which despite its name depends only on one Session's slot and
   its lecturers. Precomputed lookup tables and masks; O(1) exact deltas.
3. **Aggregate over a set** — `OnlineOnsiteSameDay` and `MaxOnlineShare`, in
   `aggregates.rs`. Neither is expressible as a slot-keyed bitset.

Within shape 3 the two types still differ, and the difference is load-bearing:

- **`OnlineOnsiteSameDay` stays a feasibility filter.** It interacts at *day*
  granularity, but it is monotone-safe: placing the first Session on a day can
  never violate it. So the search never produces one, and anything reported came
  from the caller's immovable input.
- **`MaxOnlineShare` cannot be a filter at all.** It is a cardinality ratio —
  invisible in any pair — and a filter would dead-end construction, because the
  first online Session placed makes the ratio 100% before the denominator has
  grown. Under `PER_WEEK` the denominator also *moves* when a Session relocates
  between weeks. So it lives on the **objective**, on the hard side. A run can
  therefore succeed while still reporting a `MaxOnlineShare` violation — the same
  shape as `ExactFrequency` reporting unplaced Sessions, not a new exception.

#### Group scoping differs by constraint, deliberately
- **Double-booking** propagates **both** directions (ancestors and descendants).
- **Attendance, and both Group-scoped aggregate types**, propagate **downward
  only**. A cohort Session implicates its classes' members; a class Session does
  not implicate the cohort.

#### Two search fixes found by slice 4's falsification tests
Both were real defects, surfaced by tests written to fail against a wrong
implementation rather than to confirm the right one:

- **LNS never retried Sessions that construction left unplaced**, because `ruin`
  only selected *placed* placements. That made the `unplaced` term of the
  objective permanently unoptimizable. Unplaced placements now join the repair
  list every iteration.
- **Repair broke ties by lowest index**, which made it fully deterministic given
  a candidate list and collapsed the neighbourhood: ruining the same Session
  always regenerated the same placement, so LNS could not escape a tie-induced
  dead end. Ties are now broken with the seeded RNG — still reproducible, but the
  neighbourhood is real.

#### Other notes worth knowing before changing this code
- **The objective is maintained incrementally.** Debug builds assert on every
  iteration that it matches a from-scratch recomputation. The share counters have
  a *moving denominator*, which is more error-prone than the soft sums, so the
  aggregate-drift test is the highest-value test in slice 4 — as the soft
  equivalent was in slice 3.
- **The hard penalty is derived, never tuned**: `sum(weights) x placements + 1`.
  Both `unplaced` and `aggregate` sit on the hard side and are covered by the
  same bound, so the scalar objective still orders lexicographically.
- **Search hyperparameters are not domain magic numbers.** `search::tuning` holds
  the cooling rate, stagnation limit and candidate cap. The ban here is on
  *domain* assumptions — `slot % 3`, `timeslot > 14`, `weeks[-n:]`.
- **Construction is seed-independent**; the seed influences only the LNS phase.
- **Negative soft weights and out-of-range share ratios are rejected** with
  `INVALID_ARGUMENT`, as is a `MaxOnlineShare` with no window — a ratio is
  meaningless without one.
- Known performance item for slice 5: repair enumerates `slots x eligible_rooms`
  per removed Session, sampled down to `MAX_CANDIDATES`.

## 5. Reference

Prototype: **TimeCraft**, a prior student project — Python, CP-SAT via OR-Tools.
Its constraint set is the origin of the 13 types above. Its hardcoded
assumptions (`timeslot % 3`, `timeslot > 14`, `weeks[-exam_weeks:]`, 30% online
cap) are exactly what the parametrized versions above replace — treat any
resemblance to those magic numbers in new code as a bug.
