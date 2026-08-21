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

#### KNOWN GAP — the run registry grows without bound
`runs.rs` keeps every run in an in-memory `HashMap` with **no TTL and no
eviction**, so every run ever started stays resident until the process restarts.
Not urgent at current volumes, but it is an unbounded growth path in a
long-running service.

Recorded here at the Nuxt side's request: the app had been carrying this note in
its own CLAUDE.md, which meant the one repo that can fix it was the one repo that
did not know about it.

Two consequences the app already depends on, so changing the registry means
changing them together:
- the app captures a run's result **the moment it goes terminal** rather than
  when someone asks to apply it, because "I'll fetch it later" is a promise a
  restart breaks;
- the app treats `NOT_FOUND` as **terminal and unrecoverable** (the solver
  restarted and lost the run) while `UNAVAILABLE` is transient and leaves its row
  untouched. An eviction policy would make `NOT_FOUND` mean two different things,
  and the app cannot tell them apart.

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

**`rooms()` counts virtual rooms, and that overstates exclusive capacity.** The
room axis divides by `physical_rooms + virtual_rooms`, which was right while
every room was capacity-1 and is wrong now that virtual rooms host unlimited
concurrent Sessions — they belong in neither the numerator nor the denominator of
a *contention* measure. At large-university that is 10 of 140 rooms, so the room
axis reads about **7% slacker than it should**. It has not been changed, because
the group axis binds in every preset and the room axis is not what calibration
turns on; recalibrating presets on the back of it would move every benchmark
number for no measured gain. Worth correcting if the room axis ever becomes the
binding one.

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
cargo test --workspace             # 98 tests
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

### Implementation status — slices 1-6 complete

**Done, and measured:**

- **All 14 catalogue types.** There is no longer an `UNIMPLEMENTED` branch in
  `convert.rs` — a new type added to the schema fails to compile against the
  match instead, which is the property that mattered.
- **Search**: greedy construction, then Large Neighborhood Search with
  simulated-annealing acceptance, driving `MoveEvaluator` for real.
- **Benchmark generator and harness** (`crates/gen`), calibrated on the binding
  axis and guarded against the class of infeasibility a load metric cannot see.
- **Performance**, end to end. A 27,136-Session university solves in **349 ms**,
  down from 7.79 s across slice 6: construction 229 ms, `evaluate_hard` 47 ms,
  LNS 73 ms. See the attribution table below.

**Performance work is finished.** It stopped here deliberately, not for lack of
a next idea. The remaining candidate — 6b-ii, bitset-intersecting the
room-independent axes in construction — was instrumented rather than assumed:
first-fit already exits after ~26% of the slot space while a bitset computes all
of it, so the estimate was 3-5x on construction and ~1.7x whole-run, for a larger
change than either fix that landed. Against three consecutive overestimates
(see the recurring-mistake callout), that did not justify the work on a case
already this fast. **Do not reopen it without a new measurement showing the run
is too slow in practice.**

**What remains is correctness and feature work, not performance:**

1. **v2 minimize-movement lock policy.** `LOCK_POLICY_MINIMIZE_MOVEMENT` still
   returns `UNIMPLEMENTED`. Replaces the v1 hard lock with a soft
   minimize-movement penalty so the solver *may* disturb out-of-scope Sessions
   when genuinely necessary. `Immovable` already records *why* each Session is
   immovable precisely so this is a policy change rather than a rewrite: v2
   relaxes `OutOfScope` and no other variant.
2. **The over-supply reporting gap** — see its own callout below.
3. **`MaxOnlineShare` is not enforced by the search** — see its own callout
   below, which carries a measured move-budget sweep, the `ruin_worst`
   objective-blindness finding, and two things NOT to do. Note this one is
   solution quality, not correctness: the constraint is evaluated and
   reported correctly, the search simply does little to satisfy it.

**Outside this repo entirely:** the Nuxt (`calendry`) integration session,
deferred since before slice 1. See "STILL TO DO" above — the submodule pin, the
`.npmrc` for GitHub Packages (the one part of the pipeline never exercised end to
end), and `PersonDoubleBooking` in the app's manual-edit evaluator.

#### THE RECURRING MISTAKE — measure end-to-end impact before optimizing a component
**A real, correctly-measured inefficiency can still be irrelevant to whole-system
performance if it sits in a small time slice. Always measure end-to-end impact
before optimizing a diagnosed component.**

Three independent instances in this repo, each one a correct measurement that
pointed at the wrong work:

- **H1** — repair's enumeration waste was genuinely 149x at large-university, and
  fixing it was worth **<1% of runtime**, because repair sat inside a 1% slice.
- **H2** — the retry-all-unplaced multiplier was real and dramatic, but it was a
  **symptom of infeasible instances**, not a defect to fix directly. It vanished
  on its own once the generator was corrected.
- **6b-i** — hoisting the room-independent checks gave 31x on construction, and
  the true bottleneck afterwards turned out to be `evaluate_hard`, **not** the
  room loop that 6b-ii had been scoped to attack.

The corollary that costs the most when skipped: a component's *share* of runtime
decides whether optimizing it matters, and that share **moves** every time
something else is fixed. Re-attribute after every change rather than carrying
forward the previous slice's picture.

#### MEASURED — where a run's time actually goes
Numbers from `cargo run --release -p calendry-solver-gen --bin bench`, taken
against **corrected** instances (slice 6a). The slice-5 figures were measured on
infeasible instances and are superseded.

All four presets now place **every** Session, and LNS runs properly at every
scale:

| preset | placements | iterations | improvements | unplaced |
|---|---|---|---|---|
| small-school | 1,497 | 91 | 82 | 0 |
| large-school | 3,167 | 84 | 76 | 0 |
| small-university | 6,760 | 89 | 77 | 0 |
| large-university | 27,136 | 80 | 67 | 0 |

Phase timings move as fixes land — see the corollary above. The current split,
after 6b-i and the `structural` fix:

| preset | construct | evaluate_hard | LNS | total solve |
|---|---|---|---|---|
| small-school | 3.2 ms (8%) | 0.6 ms (1%) | 35.8 ms (90%) | 39.5 ms |
| large-school | 7.6 ms (16%) | 1.6 ms (3%) | 37.0 ms (80%) | 46.2 ms |
| small-university | 25.1 ms (33%) | 6.3 ms (8%) | 43.9 ms (58%) | 75.3 ms |
| large-university | **229 ms (66%)** | 46.8 ms (13%) | 72.9 ms (21%) | 349 ms |

**There is no single bottleneck any more — it is scale-dependent.** At school
scale LNS dominates, but that is the move budget being spent, not a defect: LNS
time is roughly constant (36-73 ms) across a 18x range of instance size because
runs are budget-bound at 200k moves, so its *share* falls as instances grow. Only
at large-university does construction dominate, at 66%.

Post-fix construction scans 241.7 start slots per placement (of 924) and 238.9
room probes, at 8.2 us per placement. That cost is now almost entirely the
room-independent check''s attendee scan. **H2 is gone, at every preset size.** It was a symptom of infeasibility, not an
independent defect: with 0 unplaced there is nothing to retry, and LNS completes
80-91 iterations everywhere. **H1 is fixed but was never the lever** — repair
sits inside a 1% slice at large-university, so its 148x enumeration waste was
worth well under 1% of wall time.

#### FIXED — a virtual Room was treated as an exclusive resource

Found by a targeted audit from the Nuxt side, not by a failing test — nothing
here exercised concurrent online delivery, so it had been silently capping it
since slice 1.

`Occupancy.room` is a `BitMatrix` over (rooms x slots): binary, with no capacity
dimension. Neither `is_free` nor the `RoomDoubleBooking` branch of `check_pair`
consulted `is_virtual`, so once any Session occupied the virtual room at slot S,
**no other Session could be placed there during construction OR LNS** — one
online Session per slot, institution-wide. That constrained the SEARCH, not just
the report, which makes it worse than the app-side equivalent (fixed there
first): it changed the placements produced, not merely how they were described.

That this was an oversight rather than a stance: `is_virtual` was already
consulted by `MinimizeOnline` (`soft.rs`), the `allow_online` gate (`convert.rs`)
and `SearchState::is_online`. The proto states the intent outright — *"Online
delivery is modeled as a virtual Room rather than a boolean flag on the Session,
so room-assignment logic stays uniform."* Uniform room handling was the design;
the occupancy layer just never got the exemption everything else already had.
The generator says the same thing in its own comment ("Unbounded capacity")
while the occupancy layer capped each virtual room at one.

**One predicate, two layers, no room to drift.** `Room::is_exclusive()` is the
single definition. `Occupancy::exclusive_room()` is the only expression the
search consults, and `mark`, `unmark` and `is_free` all go through it — so they
cannot claim a bit the others do not test. `check_pair` calls `is_exclusive()`
directly. Had the two disagreed, the solver would refuse placements it then
declined to report, or free a bit it never set.

Threading `problem` into the three `Occupancy` methods was free: every caller
(`SearchState::{is_free,mark,unmark}`, `Occupancy::from_fixed`) already held it.

**Key on the FLAG, never on a well-known "online" room** — nothing restricts a
tenant to one virtual room, and the presets ship 2 to 10 of them.

Audited and found genuinely isolated to `RoomDoubleBooking`: the `lecturer`,
`attendee` and `group` matrices are marked and queried without reference to
`who.room` at all, and `check_pair`'s other three branches key on persons and
groups. That is also right on the merits — a person cannot attend two things at
once whether or not one of them is online.

**`capacity` still gates ELIGIBILITY** in `convert.rs` and was deliberately left
alone. A virtual room with a genuine concurrency limit (a single meeting licence)
cannot be expressed today at all — `capacity` means seats — and would need an
explicit `concurrent_capacity`, not an overload of this flag.

One fixture depended on the bug and was rebuilt, not patched:
`group_day_with_both_room_types` pinned a Session into the virtual room to make
it unavailable at one block, which only worked while virtual rooms were
capacity-1. It now produces its mixed day from **eligibility** — one Offering
permitted online, one not — so it cannot regress the same way.

#### KNOWN GAP — MaxOnlineShare is not enforced by the search, and never was

Fixing the virtual-room bug above **more than doubled `MaxOnlineShare` violations
at large-university, 180 -> 455**, and that one constraint type is the whole
objective regression:

| preset | aggregate before -> after | soft before -> after | unplaced |
|---|---|---|---|
| small-school | 4 -> 5 | 1,254 -> 1,288 | 0 -> 0 |
| large-school | 23-24 -> 32-33 | ~3,400 -> ~3,470 | 0 -> 0 |
| small-university | 60-63 -> 62-69 | ~7,200 -> ~7,300 | 0 -> 0 |
| large-university | **167-180 -> 448-461** | ~26,100 -> ~29,200 | 0 -> 0 |

Structural violations are **unchanged at exactly 80** (14 group + 66 person) at
large-university before and after, and `unplaced` stays 0 at every preset — so
the fix moved nothing it should not have. Objective totals rose 25-49% at school
scale and 153-176% at large-university, entirely through the hard penalty
(379,905 per violation) applied to that aggregate count.

The mechanism: virtual rooms are the **overflow valve** when physical rooms are
full at a slot — they sort last in `eligible_rooms`, so construction reaches them
last. The capacity-1 bug held that valve nearly shut, which incidentally kept
online usage down. `MaxOnlineShare` is deliberately **not** a construction filter
(a ratio whose denominator has not grown yet dead-ends construction — see the
constraint-shapes section), so with the valve open nothing bounds online usage
until LNS.

**The honest framing: this is a pre-existing weakness the fix REVEALED, not one
it caused.** The search was never enforcing that cap; a room-occupancy accident
was. The presets were calibrated in slice 5/6 against a solver whose online
capacity was accidentally limited.

Four findings below, all measured this session. **Do not act on any of them
without re-measuring** — the repo's recurring-mistake rule applies, and finding 1
is a worked example of exactly why.

##### 1. Move budget: the annealer never cools at 200k moves

Sweep at large-university, seed 1, varying `--moves` only:

| moves | iterations | aggregate | soft | solve |
|---|---|---|---|---|
| 200k (bench default) | 85 | **455** | 29,181 | 339 ms |
| 1M | 444 | **386** | 25,622 | 627 ms |
| 5M | 2,181 | **219** | 15,093 | 2.28 s |

Construction ends at ~478. The curve is **monotone and still falling steeply at
5M** (10%=159.2M -> 100%=83.2M); no plateau anywhere in the range. Wall cost
scales far better than linearly — 25x the moves for 6.7x the time — because
construction is a fixed 230 ms and iterations/second *improves* with run length
(251 -> 955/s). Per-iteration yield does decay: 0.27 -> 0.21 -> 0.12 violations
removed per iteration.

Why so few iterations, and why that is the whole story:

- **`k = 1 + rng.below(8)`** — a ruin touches ~4.5 placements.
- **`MAX_CANDIDATES = 512`** scored per repaired placement, so ~2,300 moves per
  iteration. **Moves buy candidate BREADTH, not coverage.**
- 200k moves therefore repairs roughly **380 of 27,136 placements — 1.4% of the
  instance**.
- **`COOLING = 0.999` is per ITERATION.** At ~86 iterations the temperature is
  still x0.918 of initial. Only the 5M run (2,181 iterations, x0.113) completes
  anything resembling an annealing schedule.

**This is NOT scale-dependent, which was the surprise.** Every preset lands at
85-88 iterations at 200k moves, because iteration cost is `k x MAX_CANDIDATES`
and is independent of instance size. So the 200k-move budget is an essentially
isothermal walk at *every* scale.

**Recorded as a wrong prediction, deliberately:** reasoning from the cooling
schedule alone, the expectation before measuring was that extra budget merely
extends a near-zero-temperature hill-climb and plateaus. The opposite is true —
extra budget buys the *first actual annealing*. This was not knowable without
running it, and the same will be true of the other three.

The cost of using this lever: it re-opens a performance envelope slice 6
deliberately closed (7.79 s -> 349 ms). **Spending 2.28 s where 349 ms was
celebrated must be a conscious decision, not drift.**

##### 2. `ruin_worst` is blind to 99.98% of the objective

`ruin_worst` is documented as "the placements contributing the most soft
penalty", and that was right in slice 3, when soft *was* the objective. **Slice 4
moved `unplaced` and `aggregate` onto the hard side and the operator was never
updated.** At large-university soft is 29,181 of an objective of 172,885,956 —
**0.017%** — so the arm whose job is "ruin the worst thing" is steering by a
rounding error, and the other two arms are random and related.

So LNS does not merely fail to *stumble onto* share breaches; one third of its
selection is actively aimed at the wrong quantity.

**The better fix is to correct `ruin_worst`, not to add a fourth arm.** Scoring
total objective contribution is a smaller change, removes an inconsistency rather
than working around it, and fixes the same problem for any future aggregate type.
A share-targeting fourth arm would work too and is cheap — `ruin` already takes
`state: &mut SearchState`, and `SearchState.aggregates` holds the share counters,
so the data is already in hand and dispatch is `rng.below(3)` -> `below(4)`.

Repair itself needs no change either way: resolving a breach is worth 379,905
against soft deltas of a few units, so scoring already prefers a free physical
room overwhelmingly.

**The one piece of genuine design work**: a share breach is a property of a
*group's ratio*, not of any single placement, so "which placement is responsible"
needs a convention (every online placement in a breaching group, most likely).
That is the part to think about rather than assume.

##### 3. Recalibrating the presets would re-create the same masking

Lowering `max_online_share`, or adding virtual rooms, until the numbers resemble
the pre-fix ones would tune the benchmark to what the solver currently does.
**That is the same failure mode as the bug just fixed** — a cap enforced by
accident rather than by the search — with the accident relocated from the
occupancy layer into the preset file.

**A falling violation count must never be the justification for a preset change.**
If a preset moves, it moves because the instance became more realistic, and the
number is an outcome.

##### 4. The generator lets EVERY Offering go online, including labs

`generate.rs`'s eligible-room filter is capacity + features only, and virtual
rooms are built with `capacity: u32::MAX` and **every feature**. So every virtual
room is eligible for every Offering — lectures, seminars and `KIND_LAB` alike.
There is no `allow_online` concept in the generator at all, though the wire and
`convert.rs` both have one.

That models an institution where **100% of teaching could be delivered online**,
bounded solely by a 30% share rule. No real tenant looks like that; labs are the
obvious counterexample, and the generator already labels them.

Giving the generator per-kind online eligibility is a **modelling correction**,
categorically different from finding 3 — but it would also reduce the violation
count, so it must be argued on realism and the number treated as a side effect.

##### Not a "pick one"

Findings 1 and 2 are complementary and 4 is orthogonal. Budget buys the annealing
that currently never happens without making the search smarter; ruin-selection
correctness is the right long-term fix and **stands on its own merits regardless
of this bug**. What to resist is doing 3 *instead of* 1 and 2 — the option that
makes the symptom disappear while improving nothing.

##### NOT DEFERRED, and NOT this repo: the app's default move budget

Separated from the four above because it is live rather than tracked, and because
the change belongs in **`calendry`**, not here.

The app sends `maxMoves: 50_000` with `maxWallMillis: 10_000`
(`server/api/solver/runs/index.post.ts`). That is a **quarter of the bench default
that produced every number above** — roughly 21 iterations, ~95 placements
repaired, **0.35% of a large instance**. The move budget binds long before the
wall budget, so about **9.7 of the 10 granted seconds go unused**, and 5M moves
(2.28 s at the largest preset) would fit inside the existing allowance with room
to spare.

Raising `max_moves` is also the **determinism-safe** axis: only move-budget
termination is reproducible, so spending the budget there preserves the guarantee
while leaning on the wall clock destroys it.

#### FIXED — construction re-tested room-independent axes once per Room
Of the six axes only **room occupancy** and **day-mix** (via virtual-vs-physical)
depend on which Room is being tried. Lecturer, group, person and veto read the
slot alone — yet `construct` re-tested them once per eligible Room at every slot.

`construct` now tests those four **once per slot**, before the room loop. It is a
pure short-circuit: if they reject, no Room could have rescued the slot. Output
is byte-identical — every objective, iteration count and violation count matched
exactly across all four presets.

| preset | construct before | after | speedup | mean eligible rooms |
|---|---|---|---|---|
| small-school | 31.1 ms | 3.3 ms | **9.4x** | 14 |
| large-school | 108.1 ms | 7.8 ms | **13.8x** | 17 |
| small-university | 560.4 ms | 24.9 ms | **22.5x** | 36 |
| large-university | **7.12 s** | **229 ms** | **31.1x** | 83 |

The estimate on record beforehand was "~60% floor, materially more from the
attendee-scan argument, maybe ~50x". The floor was the wrong model and badly
understated it: 60% was the share of *probes* saved, but the probes are not
equal. A room check is one early-exiting bit test; the room-independent path
scans an attendee list averaging 65 people, and that scan previously ran once per
*free* Room per slot. Speedup tracks `~0.4 x eligible_rooms` — 0.4 x 83 = 33
against 31.1x measured — which is the room-occupancy rate deciding how often the
expensive path was redundantly re-entered.

#### FIXED — `structural` pair-scanned attendee lists
`evaluate_hard` was 69% of a large-university run, and `structural` was 99.2% of
that. Attribution inside it, by successively disabling parts:

| | cost | share |
|---|---|---|
| attendee intersection, per pair | 458 ms | 72% |
| `format!` of the slot label, per pair | 160 ms | 25% |
| group closure | 6.5 ms | 1% |
| lecturer + room | 11 ms | 2% |
| views + bucketing + loop | 3.8 ms | <1% |

Both dominant costs were **unconditional**: `check_pair` allocated the location
string before checking anything, and ran all four clash searches before
consulting whether any instance covered the pair. Emptying every constraint list
changed the time by **0.2%**, so none of it was reporting.

Fixed by inverting the person axis — per slot, map each attendee to the Sessions
holding them and look for one held twice, instead of asking every pair whether
they intersect — and by making the slot label a `Display` rendered only inside a
real violation message. `structural` **623 ms -> 40.4 ms (15.2x)**, output
byte-identical.

It stays independent of `Occupancy`: it reads the same `View` attendee lists the
pairwise version read, so it remains the authoritative check rather than trusting
the index the heuristic uses to avoid violations.

#### Structural violations can only involve IMMOVABLE Sessions
Measured, both universities: **every** structural violation has two immovable
Sessions; none involves a placed one (80 of 80, and 9 of 9).

That is provable, not incidental. Occupancy is seeded from the immovable input,
repair only places where `is_free` accepts, `Enforce` is a documented
*conservative* approximation, and the mark/query semantics match `check_pair`
exactly on all four axes — group marks the closure and queries by identity, which
*is* `conflicts(x, y)`; person marks and queries attendees, which *is* set
intersection.

So the violating set is knowable before the search starts and never changes, and
~99.75% of the pairwise scan cannot report anything. **This was deliberately NOT
exploited.** Restricting the scan to immovable pairs would make the authoritative
check depend on the correctness of the thing it exists to verify — and that
safety net has already caught two real search defects. Worth revisiting only as a
debug-gated fast path, never as a replacement.

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
- Repair addresses the `slots x eligible_rooms` candidate space **by index** and
  samples `MAX_CANDIDATES` out of it with a virtual partial Fisher-Yates, rather
  than materializing the cross product. See `SlotTable::start_count` / `nth_start`.

## 5. Reference

Prototype: **TimeCraft**, a prior student project — Python, CP-SAT via OR-Tools.
Its constraint set is the origin of the 13 types above. Its hardcoded
assumptions (`timeslot % 3`, `timeslot > 14`, `weeks[-exam_weeks:]`, 30% online
cap) are exactly what the parametrized versions above replace — treat any
resemblance to those magic numbers in new code as a bug.
