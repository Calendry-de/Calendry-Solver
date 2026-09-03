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
- [ ] A Person's room pin is checked against the CHOSEN lecturers through `Problem::room_pin_blocks`, never precomputed into a per-Offering mask — a per-Offering mask is empty for a lecturer pool and silently permits everything — [ADR-0034](docs/adr/0034-a-room-pin-is-checked-against-the-candidate-not-precomputed-into-the-offering.md)
- [ ] Room exclusivity is read from `Room::is_exclusive()`, never from a room id; exclusivity BETWEEN Rooms is `Problem::footprint_siblings`, expanded on the QUERY side only — marking a sibling's bit makes overlap transitive, and it is not — [ADR-0022](docs/adr/0022-a-virtual-room-is-not-an-exclusive-resource.md)
- [ ] A preset never moves to make a violation count fall — [ADR-0025](docs/adr/0025-maxonlineshare-is-not-enforced-by-the-search.md)
- [ ] Past Sessions excluded from recalculation, always — [ADR-0008](docs/adr/0008-one-solve-mechanism-scope-plus-lock-policy.md)
- [ ] Locked and past Sessions never moved, in either version; out-of-scope Sessions are hard-locked under `LOCK_POLICY_HARD`, movable-but-penalized under `LOCK_POLICY_MINIMIZE_MOVEMENT` (the only variant v2 relaxes) — [ADR-0008](docs/adr/0008-one-solve-mechanism-scope-plus-lock-policy.md)
- [ ] Group conflict checks use precomputed ancestor+descendant sets, never a live tree walk
- [ ] Move evaluation stays behind the trait boundary — [ADR-0013](docs/adr/0013-move-evaluation-behind-a-trait.md)
- [ ] No soft term is ever negative; a preference charges what it did *not* meet — [ADR-0026](docs/adr/0026-personpreferencefit-charges-the-unmet-fraction.md)
- [ ] Group blackouts resolve through `expand_ancestry`, never `expand_subtree`/`expand_conflict` — [ADR-0027](docs/adr/0027-group-blackouts-inherit-downward.md)
- [ ] A rule relating two named Offerings uses the ONE relation mechanism — an ordered set plus a type — never a reference of its own — [ADR-0028](docs/adr/0028-a-relation-is-an-ordered-set-of-offerings.md)
- [ ] Lecturer-pool selection is built: a pool Offering's `PersonPreferenceFit` cost MUST go through `PreferenceModel::cost_for` (per-person, live), never the static `table` — `Problem::preference_cost_for_placement` is the one place that decides which, and `Offering::has_lecturer_pool` is the read — [ADR-0026](docs/adr/0026-personpreferencefit-charges-the-unmet-fraction.md)
- [ ] `LecturerVeto` for a pool Offering's CHOSEN lecturers goes through `Problem::lecturer_veto_blocks` against the per-Person masks, never through `Offering::veto_slots`, which is EMPTY for a pool (it is the fixed assignment's precomputed union). The pair `a_veto_binds_a_fixed_assignment` / `a_veto_binds_a_pool_offering` guards it; the pre-#131 refusal at conversion is gone — [ADR-0034](docs/adr/0034-a-room-pin-is-checked-against-the-candidate-not-precomputed-into-the-offering.md)
- [ ] `required_lecturer_count: 0` WITH candidates listed is refused, because the pool branch needs `required >= 1` and a zero count silently assigns EVERY candidate to every Session — the opposite of "the solver picks one". Zero with an EMPTY pool stays legitimate (a study period), and the solver cannot tell it from "not yet staffed": `required_lecturer_count` is a plain `uint32`, so absent and zero are the same bytes (Calendry #130, `crates/service/tests/zero_lecturers.rs`)
- [ ] The solver tolerates infeasible input; the app's "warn and allow" UX produces it
- [ ] A run with unplaced demand never reports `converged`: stagnation escalates instead, and an exhausted ladder reports `stagnated` — gated on `unplaced` alone, never the other hard counts — [ADR-0031](docs/adr/0031-convergence-is-never-declared-over-unplaced-demand.md)
- [ ] Every Session the run received comes back placed or in `retained_session_ids`, and no NEW placement ever starts before the reference — [ADR-0032](docs/adr/0032-the-answer-accounts-for-every-session-it-was-given.md)
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

cargo test --workspace               # 739 tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked

CALENDRY_SOLVER_ADDR=127.0.0.1:50051 cargo run -p calendry-solver
```

CI runs all of the above plus a release benchmark smoke run
([ADR-0020](docs/adr/0020-workspace-lints-and-ci-are-the-gate.md)). The rustdoc
line is listed above because it is the one gate the other three cannot stand in
for: `broken_intra_doc_links` is a rustdoc lint, so a doc comment left pointing
at a renamed item passes `test`, `clippy` and `fmt` and fails CI. Lints live in
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
    [--calibrate] [--diagnose N] [--evaluate] [--elective RATIO] \
    [--preferences RATIO]
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

**Every catalogue constraint type the schema defines is implemented.**
`PersonPreferenceFit` was the last of the original set — it arrived in schema
v0.7.0 and was refused as `UNIMPLEMENTED` until it was built
([ADR-0026](docs/adr/0026-personpreferencefit-charges-the-unmet-fraction.md)).
`GroupVeto` arrived with the `Group.blackouts` commit (tagged `v0.8.0` once
published) and was built in the same change as the schema field it reads, so it
never had an `UNIMPLEMENTED` phase
([ADR-0027](docs/adr/0027-group-blackouts-inherit-downward.md)).
What is still refused is one *parameter* of it: a non-empty
`PersonPreferenceFit.roles`, because the counted set is lecturers only and
widening it silently is the failure that decision exists to prevent. The
conversion layer's match is exhaustive with no `_ =>` arm, so a new type in the
schema is a compile error rather than a silently ignored setting — which is the
property that mattered, and is how `PersonPreferenceFit` announced itself when
the 0.7.0 pin landed.

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

**`LOCK_POLICY_MINIMIZE_MOVEMENT` (v2) is built.** An out-of-scope Session
becomes a movable `PlacementVar` carrying its `original` slot and room instead
of hard-locked `FixedSpec` occupancy, charged `SolveScope.minimize_movement_weight`
if the search leaves it — the ordinary per-placement soft-cost shape, same
`Objective::soft` field every other exact-delta term uses.
[ADR-0008](docs/adr/0008-one-solve-mechanism-scope-plus-lock-policy.md) covers
the shape; `Immovable` already recorded *why* each Session is immovable, which
is what made this a policy change rather than a rewrite — it relaxes exactly
the `OutOfScope` variant, and only when the Session realizes a real Offering
(an ad-hoc Session has no Offering to attach "movable" to, and stays
hard-locked under either policy). Construction seeds a movable Session back at
its original placement when nothing conflicts, so the search does not
gratuitously pay the penalty for a move nobody asked for.

**In-scope stay-put pressure is built (issue #58).** A reused IN-scope
Session now ALSO carries an `original`, charged by the independent
`SolveScope.minimize_inscope_movement_weight` rather than
`minimize_movement_weight` — the two never charge the same placement, since
`Problem::movement_cost` picks between them by the placement's Offering's
scope. Closes the measured gap (36–100% churn on a targeted repair) without
the larger, session-level-scope redesign the tracking card also considered;
see ADR-0008's "In-scope stay-put pressure, landed" addendum for why.

**`MinimizeSpecializedRoomUse` is built.** A Room marked
`Room.is_specialized` — a lab, computer room or workshop — is discouraged from
hosting teaching that does not need it, so it stays free for teaching that
does. EXEMPT BY REQUIREMENT: no charge when the Offering's
`required_room_features`/`room_feature_requirements` intersect that Room's
`feature_tags`, so the programming class in the computer lab pays nothing.
Deliberately NOT `MinimizeRoomRank`: `rank` is ordinal desirability whose
`invert` mode means "prefer the premium rooms", so encoding a lab as high-rank
would pull Sessions INTO it — and rank is kind-scoped, so it cannot tell the
class that needs the lab from the one that merely landed there. Flat, charged
once per placement, and it PRICES rather than filters (a specialized Room stays
eligible, so it is still used when it is the only one that fits). Every
decision is precomputed into `Offering::charged_specialized_rooms`, keeping
`Problem::specialized_room_cost` a bit test. See ADR-0024's addendum for why a
second room-axis type does not violate one-type-per-axis.

**Room exclusivity groups are built (Calendry #122).** Movable walls: rooms
1.0, 1.1 and 1.2 behind folding partitions are three bookable Rooms closed and
one Audimax open, so booking any one of them must make the other three
unbookable for that slot. `Room.footprint_tags` is an open-vocabulary tag,
SYMMETRIC by construction — a Room may carry several (a wall shared between
two combination options), and a tag only one Room carries is inert rather than
an error, so a half-entered configuration does not fail a run. Structural and
HARD, reported under `RoomDoubleBooking` with a message naming both Rooms
rather than as a type of its own: the rule is unchanged, only the definition
of "the same room" widened. The one refusal is a tag on a VIRTUAL Room, whose
occupancy row is never consulted (ADR-0022) — it could only be inert, and an
inert exclusivity reports no violation while double-booking the space every
time. THE THING NOT TO UNDO: the footprint is expanded on the QUERY side, in
`is_free`, never in `mark`. Marking the siblings is shorter and passes every
test but one; it makes overlap TRANSITIVE, so with `A | mid | B` behind two
separate walls, booking `A` would block `B`. See ADR-0022's third addendum.

**The spare bank crosses the wire (issue #22).** A Session with no
`start_slot` is no longer refused: it is teaching that is OWED but unplaced,
after a cancellation. It reuses its Session id and carries **no** `original`,
so it claims one of its Offering's outstanding occurrences, keeps its
identity, and is placed **free of any movement charge** — charging it would
bias the search away from rescheduling the very Session the bank exists to
reschedule. It adds no demand: it IS one of `required_session_count`, not an
extra. `reusable` now sorts **placed before banked**, then by id, because a
placed Session forfeits its `original` too when occurrences are scarce. Four
cases, not one: in-scope resolves → banked; out-of-scope or unresolvable →
ignored; no Offering at all → still refused. `is_locked` on an unplaced
Session is refused rather than guessed — see ADR-0008's "spare bank" addendum.

**Per-entity movement overrides are built (issue #70).** Both weights above
are run-wide; `SolveScope.movement_overrides` is the per-entity exception —
`{ oneof target { person_id | group_id }, weight }`, and a matching entry
REPLACES whichever run-wide weight would have applied. Still SOFT: unlike a
Session `lock` it can never prevent a move, only price it. One number covers
both settings the ticket asked for (`0` is "movable, no extra cost" even under
a large run-wide weight; a large value is soft-unmovable). A `person_id`
covers Sessions that Person LECTURES only (ADR-0026's scope decision); a
`group_id` binds that Group and its DESCENDANTS, so the query walks up through
`expand_ancestry` exactly as `GroupVeto` does (ADR-0027); the LARGEST matching
entry wins. All resolved once per Offering into
`Problem::offering_movement_weight`, so `movement_cost` stays one indexed
read — which is also why a lecturer POOL is covered by ANY matching candidate
rather than exactly. See ADR-0008's "Per-entity movement overrides" addendum.

**Lecturer-pool selection is built.** `candidate_lecturer_ids.len() >
required_lecturer_count` is a genuine choice, not a refusal: construction and
repair enumerate every valid combination the same way they already enumerate
Room combinations (`Offering::lecturer_choice_count`/`lecturer_choice`,
mirroring `room_choice_count`/`room_choice`), and `Placement::lecturers`
remembers which one a Session got, since a pool Offering's `Offering.
lecturers` is empty — there is no fixed assignment to fall back on. See
ADR-0026's "Lecturer-pool selection landed" addendum for what this did to
`PersonPreferenceFit`'s precomputed table (the trap the ADR had already
named). `LecturerVeto` was the one instance of that trap left unfixed — a
pool combined with it was REFUSED at conversion — until Calendry #131, when a
real tenant's two co-teaching lecturers hit the refusal; it now reads the
CHOSEN lecturers' per-Person masks live (`Problem::lecturer_veto_blocks`),
ADR-0034's shape one axis over. The ticket's own proposal, a "choose one
lecturer per Offering, then place" pre-pass, was NOT built: it would need a
heuristic that cannot see the placement it decides ahead of, and it would
make `LecturerConsistency` structurally inert for every pool Offering. The
live check keeps per-Session choice and lets each person's own calendar
decide. Part A of that ticket (a per-Group "Klassenlehrer" pin) is app-only.

**`OfferingRelation` (ADR-0028) and every relation type on it are built.** A
relation is an ordered set of Offering references plus a type —
`RelationSpec`/`RelationKind` on `ProblemSpec`, resolved by `Problem::build`,
independent of `ConstraintSet` because a relation names specific Offerings,
never a kind. Six kinds exist, and they split cleanly into two enforcement
styles, which is the thing to know before adding a seventh:

* **Occupancy FILTERS — the search cannot violate them.** `DifferentTime` (no
  two members ever share a slot) is the same shape as the four structural
  double-booking types: a bit shared by every member Offering in `Occupancy`'s
  relation matrix, checked in `mark`/`unmark`/`is_free`. `MeetTogether`
  (members share ONE Room at one same-week slot, with their `min_capacity`
  SUMMED against the Room's) is the other, via
  `Occupancy::meet_together_anchor`/`meet_together_cells`. Both also keep an
  independent `constraints` check, for the ADR-0014 reason every structural
  type has one.
* **HARD but PRICED at `hard_penalty`, read fresh off the solution.**
  `SameTime`/`SameDays`/`SameStart` compare per-week SETS of `(day, block)` /
  day / block for equality; `Precedence` (issue #37) compares the
  predecessor's latest end against the successor's earliest start. Neither
  question is decidable mid-search from partial state, so neither can be a
  filter — a run can succeed while reporting one of these. Same stance
  ADR-0025 records for `MaxOnlineShare`.

`Precedence` is the ONLY kind that reads member order, which is what ADR-0028
kept the set ordered for — and it is now the only kind that ever will, since
the `NextDay`/`TwoDaysAfter` family turned out to be its parameters. It is
term-wide and all-pairs (all lectures finish before any lab starts — not a
per-week pairing, not `UniTime`'s first-meetings-only), it decides ordering
structurally but measures `min_gap_minutes` in wall-clock minutes through
`GridTime` and both `min_days_between` and `max_days_between` in CALENDAR
days, and it counts LOCKED Sessions where the `SameTime` family does not. All
four decisions, and the one open divergence, are in ADR-0028's "`Precedence`
landed" addendum; the day FLOOR and what it deliberately cannot say are in the
day-floor addendum below it.

THE ONE EXCLUSIVITY TO KEEP: a day-floor breach SUPPRESSES the minute-gap
check, the way `OutOfOrder` suppresses both — a boundary on the wrong day has
no meaningful minute gap to be short, and `Objective::hard` sums the violation
count, so charging one mistake twice mis-prices it. It does NOT suppress the
ceiling: under `min_days_between > max_days_between` both bounds are genuinely
breached and the timetabler needs to see both.

**Convergence is never declared over unplaced demand (issue #120,
[ADR-0031](docs/adr/0031-convergence-is-never-declared-over-unplaced-demand.md)).**
`termination_reason: "converged"` is reserved for a best solution with ZERO
unplaced Sessions. While demand remains unplaced, hitting the stagnation limit
ESCALATES instead of terminating — the ruin cap doubles per level
(`tuning::RUIN_CAP_BASE`/`ESCALATION_LEVELS`, 8 → 64) and the temperature is
reheated — and only an exhausted ladder stops the run, with the new reason
`"stagnated"` (a plain wire string, no proto change; the app must not read an
unknown reason as success — #119's reporting work). The ladder is FINITE, so
an unbudgeted call still terminates, and it is gated on `unplaced` alone,
never the other hard counts — ADR-0025's stance on the aggregate hard terms
is unchanged. Three search changes travel with it: a fourth ruin arm,
`ruin_blocking`, that probes an unplaced Session's candidate cells and evicts
the cheapest movable blocker set (NOT the arm ADR-0025 rejected — an unplaced
Session has no placement whose scoring could be fixed); rounds now repair
previously-unplaced Sessions FIRST, each class seeded-shuffled, because the
old ascending-index order handed every contested freed cell to the lowest
index forever; and an unplaced Session whose SAMPLED candidates all score
infeasible gets an exhaustive `is_free` fallback over its full candidate
space, so "no placement exists" is a true statement rather than a sampling
artifact.

**The answer accounts for every Session it was given (ADR-0032, "the
vanishing eleven").** A Session before `reference_slot` classifies as PAST,
satisfies its Offering's demand as fixed occupancy, and used to come back in
NEITHER `sessions` NOR `unplaced_offerings` — the app deleted taught history
as `not_returned_by_solver`, and nothing stopped the replacement landing in
an elapsed week for the NEXT run to drop. Two changes close the loop:
`SolverOutput.retained_session_ids` lists every non-Federation immovable
Session the run kept (the applier's rule: gone = in neither list; retained
Sessions are still deliberately NOT echoed as placements), and
`ProblemSpec::reference` masks every candidate start before the run's "now"
in `SearchState::statically_blocked` — one definition covering construction,
repair and `ruin_blocking`. Core `reference: None` masks NOTHING (fixtures,
generator); the wire's "no reference / beyond the term" case maps to
one-past-the-last-slot and masks EVERYTHING, matching what
`classify_immovable` already said it meant.

**A fixed Room for a Person is built (Calendry #124 v2, [ADR-0034](docs/adr/0034-a-room-pin-is-checked-against-the-candidate-not-precomputed-into-the-offering.md)).**
"The workshop lead always teaches in the workshop." `Person.allowed_room_ids`
plus a `LecturerRoomPin` switch — the `Person.blackouts` + `LecturerVeto`
split, because a bare field has nowhere to put `applies_to_kinds`. HARD and a
FILTER in `SearchState::statically_blocked`, not priced: it is candidate-local
and monotone, so none of ADR-0023/0025's priced-hard reasons apply, and the
soft reading of this axis already shipped as
`Preference.preferred_room_features` (which two documents wrongly called
unbuilt until this change). THE THING NOT TO UNDO: the pin is checked against
the placement's CHOSEN lecturers via `Problem::room_pin_blocks`, NEVER
precomputed into a per-Offering mask. `LecturerVeto` WAS the tempting sibling
and the wrong one — its mask came from `Offering::lecturers`, which is empty
for a genuine pool, which is exactly why `LecturerVeto` + a pool had to be
refused until #131 gave it this shape. A per-Offering mask here passes every fixed-assignment test and is
silently permissive for the pool case the feature exists for; the guard is the
mirrored pair `a_pin_binds_a_fixed_assignment` /
`a_pin_binds_a_pool_offering`. Also: the whitelist is inverted ONCE in
`Problem::build` into `person_room_veto`, so "empty blocks nothing" holds as
it does for every other mask; EVERY Room of a multi-room Session must satisfy
the pin, since "at least one" lets a hard rule be escaped by asking for more
Rooms; and a pin never expands through `footprint_siblings` — ADR-0022 expands
a BLOCKING question, and a permission never expands. A pin on a VIRTUAL Room
is honoured, not refused, which is the opposite call to
`FootprintOnVirtualRoom` and one word from being misapplied.

**A scoped exam week is built (Calendry #126 sub-ask 3, [ADR-0033](docs/adr/0033-an-exam-week-is-scoped-on-the-calendar-and-charged-per-offering.md)).**
A calendar period was term-global, so two cohorts sitting exams in different
weeks was unsayable. `Week.exam_group_ids` narrows the week — empty means
every Group, so every earlier pin keeps today's behaviour exactly — and
`MinimizeExamWeek` LEAVES `SoftParams` as a result: its predicate now reads
which cohorts attend the Offering, and the soft table is keyed by a
kind-profile that cannot express that. It joins the near-miss family instead
(`Offering::exam_week_slots`, `Problem::exam_week_cost`), which is ADR-0026's
move for `PersonPreferenceFit` one axis over. THE THINGS NOT TO UNDO:
`hard_penalty` gained `exam_week_weight * placements`, because the type
leaving `ConstraintSet::soft` removed its contribution from
`soft.total_weight` and the bound would otherwise shrink SILENTLY; and the
mask is NOT emptied when the charges are zero the way
`charged_specialized_rooms_for` empties its own — an empty exam mask under
`invert` means charged at EVERY slot, not free, so the guard is on the
charges. `SlotFlags` is unchanged and an exam week is still OPEN, which is the
property the whole design exists to protect. Verified byte-identical on all
four presets, objective totals included.

**`LecturerConsistency` is built.** Once its prerequisite (lecturer-pool
selection) landed, the remaining gap was one evaluator: a distinct-lecturer
count over an entire Offering's placed Sessions, priced against
`Offering::lecturer_required_count()` — `max(0, distinct_lecturers -
required)`, the same shape `RoomConsistency` uses for the Room axis but
keyed by lecturer identity, in `Aggregates::lecturer_rows` (a small
per-Offering histogram, not a dense `offering * person` matrix, since only a
genuine pool Offering can ever have a nonzero row). A fixed assignment's
distinct count never changes, so this type can never fire for the
overwhelming majority of Offerings. The manual per-Session override the
tracking card also named is app-side and still unbuilt.

Deliberately not built:

* **A GPU move-evaluation backend.** The seam exists and has two adapters; the
  backend does not. [ADR-0013](docs/adr/0013-move-evaluation-behind-a-trait.md).
* **`CanShareRoom`** — and it is REFUSED rather than pending, with the
  reasoning executable in `crates/core/tests/can_share_room.rs` and
  [ADR-0035](docs/adr/0035-room-sharing-is-a-property-of-the-room.md). Its
  honest reading is UniTime's PERMISSION primitive, which is an exemption with
  no rule behind it: it can never be violated, so it has no evaluator and
  nothing to report, and it puts a hole in a hard structural type that
  ADR-0014 then obliges `check_pair` to mirror. The load-bearing finding is
  that **`MeetTogether`'s `(relation, week)` anchor is chain-transitive** —
  with `{A,B}` and `{B,C}`, `C` joins `A`'s cell — which is CORRECT, because
  "is the same physical meeting" is an equivalence relation, and wrong for a
  permission, which is only symmetric. So building it on that machinery is
  ADR-0022's transitivity bug again, and building it properly needs per-cell
  occupant identity `Occupancy` deliberately does not carry. The
  capacity-relief reading ("this hall holds two seminars at once") is a ROOM
  axis — `Room::is_exclusive()` is `!is_virtual`, so no non-exclusive physical
  Room exists — and the preference reading is UniTime's separate `SameRoom`,
  a different unbuilt kind that stays cheap if anyone asks.
* **The "N hours between" family is not coming either, because it is already
  here.** `Precedence.min_gap_minutes` says it exactly, and the day-counted
  half is one scalar — `Precedence.min_days_between`, the FLOOR to
  `max_days_between`'s ceiling — not `NextDay`/`TwoDaysAfter` kinds, which
  would be the constants `1` and `2` welded into type names (ADR-0024). The
  floor exists because a wall-clock value CANNOT express a day boundary: the
  separating threshold is a function of the grid, and on a teaching day
  spanning 12 hours or more no value works at all. Two readings stay refused
  and both are pinned in `crates/core/tests/day_counted_relations.rs`: "the
  next TEACHING day" is `Daybreak`'s unit rather than this type's (and
  `min == max` in calendar days makes a Friday predecessor demand a
  Saturday), and UniTime's PER-OCCURRENCE pairing is a new kind rather than a
  parameter, since `Precedence` declares itself term-wide and all-pairs. See
  ADR-0028's day-floor addendum.

Outside this repo: the Nuxt integration session, including the one part of the
schema pipeline never exercised end to end. See
[`docs/SCHEMA.md`](docs/SCHEMA.md).

---

## Four constraint shapes — read before adding a type

1. **Pairwise, keyed by `(entity, slot)`** — the four structural double-booking
   types. Occupancy bitsets; the search can never violate them. Note that the
   room axis exempts non-exclusive Rooms, and that a booked Room's bit is
   tested against every Room sharing its physical footprint
   ([ADR-0022](docs/adr/0022-a-virtual-room-is-not-an-exclusive-resource.md)).
2. **Unary, keyed by `(slot, room)`** — most soft types, and also
   `LecturerVeto`, which despite its name depends only on one Session's slot and
   its lecturers. Precomputed lookup tables and masks; O(1) exact deltas.
   Note the near-miss family that does NOT fit here: `MinimizeCapacityWaste`,
   `MinimizeBreakSpanning`, `MinimizeSpecializedRoomUse` and — since
   [ADR-0033](docs/adr/0033-an-exam-week-is-scoped-on-the-calendar-and-charged-per-offering.md)
   — `MinimizeExamWeek` are unary in the same sense but read the OFFERING (its
   `min_capacity`, its duration, its required features, its Groups), so a table
   keyed by kind-profile cannot express them. Each is a plain formula on
   `Problem` instead, summed into `Objective::soft` like any table hit. The
   membership of this shape is therefore not fixed, and counting the
   `SoftParams` variants is not a check on anything — the boundary is the
   predicate's inputs.
3. **Aggregate over a set** — `OnlineOnsiteSameDay` and `MaxOnlineShare`, in
   `aggregates.rs`. Neither is expressible as a slot-keyed bitset, and **neither
   is a filter any more**.
4. **Per-placement, keyed by `(placement, day, block)`** — `PersonPreferenceFit`
   alone, in `preferences.rs`. It is *unary* in the sense that matters (its cost
   depends on the candidate and nothing else already placed, so it accumulates as
   an exact delta and `ruin_worst` can rank it), but it cannot share shape 2's
   table: that table is keyed by a *profile*, the instance set applying to one
   tenant `kind`, and a preference cost depends on **who leads this placement**.
   One profile per distinct preference signature is one profile per placement.

   The key drops the week axis, and that is the whole reason the table is
   affordable: a preference is a recurring weekly shape, so `placement × (day,
   block)` holds the same information as `placement × slot` in 1.1 M entries
   instead of 25 M. It is only valid while a placement's lecturer set is fixed
   before the search starts — true for every non-pool Offering, which stays on
   this table unchanged. Lecturer-pool selection **is built** (issue #61): a
   pool Offering's lecturer set is a search-time choice, so it bypasses this
   table entirely and prices live over a per-person table instead
   (`PreferenceModel::cost_for`) — `Problem::preference_cost_for_placement` is
   the one place that picks which path a placement gets. See
   [ADR-0026](docs/adr/0026-personpreferencefit-charges-the-unmet-fraction.md),
   which also records why the term charges the **unmet** fraction rather than
   rewarding the met one, and why `hard_penalty` must count
   `weight × MAX_WEIGHT_MULTIPLIER` per placement rather than `weight`.

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
* **`GroupVeto` blackouts bind downward, so the QUERY walks up.** A window
  declared on a Group binds that Group and its descendants — a programme
  suspended for a period takes its cohorts with it — so a Session attached to `g`
  is blocked by the windows of `{g} ∪ ancestors(g)`. That is a third table,
  `GroupClosure::expand_ancestry`, and neither of the two above: `subtree` points
  the wrong way and `conflict` contains it plus every descendant, so both would
  let one seminar's absence veto the lecture its whole cohort attends. All three
  agree on a flat hierarchy, which is why the guard is a pair of tests over a
  two-level fixture and an ADR — `expand_conflict` fails exactly ONE of the eight
  tests in `group_veto.rs`. [ADR-0027](docs/adr/0027-group-blackouts-inherit-downward.md).
* **A scoped EXAM week binds downward too, so its query also walks up.**
  `Week.exam_group_ids` names the Groups a week is an exam week FOR, and an
  Offering's exam period is the union over `{g} ∪ ancestors(g)` for its Groups
  — the same `expand_ancestry` table, one axis over. Same guard shape, and one
  argument stronger than `GroupVeto`'s: under `MinimizeExamWeek { invert: true }`
  a wrong expansion does not over-block, it *steers*, actively pulling a
  cohort's lecture into a seminar's exam period.
  [ADR-0033](docs/adr/0033-an-exam-week-is-scoped-on-the-calendar-and-charged-per-offering.md).
* **`PersonPreferenceFit` does not propagate at all.** It counts a placement's
  *lecturers*, never its attendees, so no Group axis enters it. That is a scope
  decision rather than an omission: an attendee set averages ~65 people at
  benchmark scale, so counting them would let a 200-student cohort's aggregate
  preference outweigh the person teaching.

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
* **Three search defects were found by falsification/field evidence**, and all
  are worth knowing because the fix is easy to undo by accident: LNS never
  retried Sessions construction left unplaced (so the `unplaced` term was
  permanently unoptimizable), repair broke ties by lowest index (which
  collapsed the neighbourhood — ruining the same Session always regenerated
  the same placement), and a round's removal set was repaired in ascending
  index order (the same collapse one level up — every contested freed cell
  went to the lowest index, every round, forever; issue #120/ADR-0031). Ties
  and repair order are now driven by the seeded RNG: still reproducible, but
  the neighbourhood is real.

---

## Reference

Prototype: **TimeCraft**, a prior student project — Python, CP-SAT via OR-Tools.
Its constraint set is the origin of this catalogue. Its hardcoded assumptions
(`timeslot % 3`, `timeslot > 14`, `weeks[-exam_weeks:]`, a 30% online cap) are
exactly what the parametrized versions replace — treat any resemblance to those
magic numbers in new code as a bug.
