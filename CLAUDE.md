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
| `crates/gen` | `calendry-solver-gen` | Benchmark generator — **placeholder, slice 5** | the service |

Conversion is deliberately *not* its own crate yet; promote it when its
validation logic grows. Correctness fixtures are hand-written in
`core/src/testing.rs`, kept separate from the generator on purpose.

### Locating calendry-proto
`.proto` files are **never vendored here**. `crates/proto/build.rs` resolves the
checkout in this order:
1. `CALENDRY_PROTO_DIR` env var (for CI)
2. `proto/` inside this repo — the **git submodule**, which is the intended
   steady state. A submodule is how a *language-neutral* proto repo gets pinned
   to a revision, since it has no `Cargo.toml` for cargo to depend on directly.
3. `../calendry-proto/proto` — sibling checkout.

**Currently path 3 is what works**: as of 2026-08-14 the calendry-proto repo has
the three `.proto` files written but **not committed and not pushed**, so there
is no revision to pin a submodule to yet. Switch to the submodule once it is
pushed.

```bash
cargo test --workspace          # 26 tests
cargo clippy --workspace --all-targets
CALENDRY_SOLVER_ADDR=127.0.0.1:50051 cargo run -p calendry-solver
```

Example request payloads live in `examples/`. The service does **not** expose
gRPC reflection, so `grpcurl` needs the proto files explicitly:

```bash
grpcurl -plaintext -import-path ../calendry-proto/proto \
  -proto calendry/solver/v1/service.proto \
  -d @ 127.0.0.1:50051 calendry.solver.v1.SolverService/StartRun \
  < examples/forced_unique.json
```

### Implementation status
**Slice 1 complete.** Implemented: `StartRun`/`GetStatus`/`CancelRun`, in-memory
run registry, both budgets, seeded determinism, past/locked/out-of-scope
immovability, and **two** constraint types — `RoomDoubleBooking` and
`ExactFrequency`. Search is **greedy construction only**; `MoveEvaluator` +
`CpuEvaluator` exist and are tested, but no metaheuristic drives them yet.

The two-constraint pairing is deliberate: room double-booking alone is **not
falsifiable**, because with nothing forcing placement an empty schedule
satisfies it vacuously. Exact frequency supplies the placement pressure.

Everything unimplemented returns an explicit `UNIMPLEMENTED` — no enabled
constraint is ever silently ignored, since that would make a schedule look
validated when it was not.

Next slices: (2) remaining structural types + group closure with
ancestor/descendant sets; (3) SA/LNS + the six soft types + real objective;
(4) remaining hard types; (5) benchmark generator.

## 5. Reference

Prototype: **TimeCraft**, a prior student project — Python, CP-SAT via OR-Tools.
Its constraint set is the origin of the 13 types above. Its hardcoded
assumptions (`timeslot % 3`, `timeslot > 14`, `weeks[-exam_weeks:]`, 30% online
cap) are exactly what the parametrized versions above replace — treat any
resemblance to those magic numbers in new code as a bug.
