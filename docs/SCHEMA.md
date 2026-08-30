# Schema distribution, and what remains

Status, not decisions. The decision this records is
[ADR-0003](adr/0003-proto-schema-as-a-pinned-submodule.md) — the schema lives in
a separate repo, consumed as a pinned submodule.

## Current pin: `03aed98` = `v0.10.0`

`SolveScope.minimize_movement_weight` (field 4) was added on top of `855c145`
while building "v2 minimize-movement repair mode" — see the correction below,
which this pin bump directly falsifies. Tagged and pushed to
`github.com/MindCollaps/calendry-proto`; `publish.yml` ran for real on the tag
push (run `33235914770`, not a dry run — verified from its own job log, same as
`v0.9.0`), so `@mindcollaps/calendry-proto@0.10.0` is on GitHub Packages.

* **`SolveScope.minimize_movement_weight`**, a plain `double`, not `optional` —
  `0.0` is `LOCK_POLICY_MINIMIZE_MOVEMENT`'s own "track it, do not steer"
  reading, the same one every other soft weight already gives a zero. Solver-side:
  **done** — `crates/service/src/convert.rs` reads it, rejects negative/NaN
  (`ConvertError::NegativeMovementWeight`), and `Problem::movement_cost` prices
  it. See [ADR-0008](adr/0008-one-solve-mechanism-scope-plus-lock-policy.md)'s
  "v2, landed 2026-08-29" section for the mechanism and the decisions that were
  not obvious from the enum alone.

## Previous pin: `855c145` = `v0.9.0`

Published 2026-08-29: `@mindcollaps/calendry-proto@0.9.0` is on GitHub
Packages (`publish.yml` ran for real on the tag push, not a dry run — verified
from its own job log, not assumed from a green check). Batched deliberately —
every field below except `placement_ref` is **PROTO ONLY**, staged
schema-first ahead of its evaluator the way ADR-0026 staged
`PersonPreferenceFit`, so one tag covers the whole set rather than one per
backlog card.

**Correction carried forward from `v0.8.0`:** that section previously said
`992563f` was "not yet tagged" and asked for the tag to be cut. It was already
stale when read — the tag existed and had all along, tagged minutes after the
commit; the entry just never got updated once it landed. Caught while adding
`placement_ref` below, checked directly against the submodule rather than
assumed from this file, since this is exactly the drift `CLAUDE.md` warns
about for a tracked-gap entry.

* **`PlacedSession.placement_ref`** (field 9). A `ConstraintViolation.
  session_ids` entry names a Session by `Problem::placement_label` — the real
  `session_id` when one exists, otherwise `offering_id#occurrence` for a
  Session this run invented. Nothing on the wire carried that same label for
  an invented Session, because `PlacedSession.session_id` is deliberately
  empty for one ("empty = newly created"), so a violation naming one pointed
  at nothing else in the response. `placement_ref` carries the label
  unconditionally, on every Session, so a violation is always resolvable to a
  concrete entry in `sessions`. Solver-side: **done**, wired into
  `build_output`. "Solver violations naming Sessions the solver invented".

* **`Room.feature_quantities`** and **`Offering.room_feature_requirements`**
  (with `RoomFeatureQuantity` / `RoomFeatureRequirement`). Today's
  `feature_tags` / `required_room_features` are presence-only, so "needs 24
  workstations" degrades to "needs a workstation". The new fields carry a
  count on both the supply (Room) and the demand (Offering) side;
  `min_quantity` is `optional` for the same zero-vs-absent reason
  `Preference.weight_multiplier` is. Solver-side: not started — eligibility
  still checks `feature_tags` membership alone. "Equipment quantity cannot
  cross the wire".

* **`Session.room_ids`** and **`PlacedSession.room_ids`**, plus
  `Offering.required_room_count`. `room_id` (singular) remains the primary
  Room and is unchanged for a single-room Session; the plural field carries
  the full set, `room_id` included, only when more than one Room is occupied
  simultaneously. Solver-side: not started — the search assigns exactly one
  Room per placement regardless of `required_room_count`. "A Session with more
  than one Room cannot cross the wire".

* **`Offering.scheduling_pattern`** (`SchedulingPattern`: distributed vs.
  block/intensive). Metadata only — nothing reads it yet, so every Offering
  solves exactly as it does today regardless of what is set. Which
  enforcement shape this takes (a per-Offering aggregate vs. a constraint type
  per pattern) is still open; this stages the classification data without
  committing to that answer. "Scheduling pattern per Offering".

* **`MinimizeExamWeek.invert`**. One flag, not a new type — the same
  `MinimizeRoomRank.invert` / `MinimizeBlockUsage` precedent. `false` (absent)
  is today's only prior behavior, unchanged; `true` pushes Sessions toward the
  exam period instead of away from it. **Solver-side: done**, 2026-08-29 —
  `SoftParams::MinimizeExamWeek` now carries `invert` and
  `crates/service/src/convert.rs` reads `p.invert` from the wire; the
  direction-flip is covered by a falsification test
  (`inverted_minimize_exam_week_steers_into_the_exam_week`, on a grid built
  specifically so the unweighted default sits OUTSIDE the exam week, or a
  dropped `invert` would pass by never having to move). "Exam-specific
  placement logic" (the wire half only; the lecturer-facing "create my own
  exam" flow, and pushing toward TERM-END specifically rather than just
  "the exam period", are still app-side / unbuilt).

* **`Compactness`** is built — see the solver repo's CLAUDE.md.
  **`LecturerConsistency`** (`oneof params` entry 30) is now built too: its
  prerequisite, genuine lecturer-pool selection, landed first (issue #61,
  `crates/core/src/preferences.rs`'s `cost_for`), and the type itself —
  "Lecturer consistency across an Offering's Sessions" — is a distinct-count
  aggregate over an Offering's whole placed set, `crate::aggregates::
  LecturerConsistencyInstance`. Only ever priced for a genuine pool Offering;
  a fixed assignment's distinct lecturer count never changes.

* **`Preference.preferred_room_features`**. Rides the existing
  `person_preference_fit` tenant-level switch, per
  `per-person-preferences-design.md` §1's own criterion (grid-shaped, widens
  the row rather than needing a new table). References `Room.feature_tags`'
  vocabulary by key, the same tradeoff `required_room_features` already
  accepts. Solver-side: not started — `PersonPreferenceFit` counts
  `days`/`blocks` only. "Room-type preference kind".

* **`SolverOutput.candidates`** (`SolverCandidate`). Marked **DRAFT — NOT
  COMMITTED** in the proto itself, not just here: unlike everything above,
  "Multiple candidate schedules" was never scoped, and naming / de-duplication
  / whether one run can produce this cheaply are all still open. The field
  exists so the design conversation has a concrete shape to react to, not
  because the shape is decided. `sessions` / `hard_violations` / `objective`
  on `SolverOutput` are unchanged and remain authoritative for every existing
  caller.

**Correction: `LOCK_POLICY_MINIMIZE_MOVEMENT` was believed to need no proto
change at all**, checked while surveying the P0 backlog for wire gaps — the
enum value already existed (`model.proto`'s `LockPolicy`, value 2). That held
right up until implementation started and found the enum had nothing to weigh
a disturbance by, unlike every other soft term on the wire. See the pin section
above: it needed exactly one field, `SolveScope.minimize_movement_weight`.

Previously `6107eb2` = **`v0.7.0`**, up from `v0.2.0`. What arrived across those
that this repo cares about:

* **`Group.blackouts`** and the **`GroupVeto`** constraint (`v0.8.0`). A Group can
  be away for part of a Term — the cohort that runs only the first six weeks. It
  reuses `Unavailability`, so absence keeps one convention across `Person` and
  `Group`, and the app sends the COMPLEMENT of the availability window it stores.
  The one thing here that is a real decision rather than plumbing is the
  DIRECTION: a window binds the Group and its descendants, so the query walks up
  — [ADR-0027](adr/0027-group-blackouts-inherit-downward.md). No
  `UNIMPLEMENTED` phase, because unlike `PersonPreferenceFit` the field and its
  evaluator shipped in the same change.

* **`MinimizeBlockUsage`** and an `invert` flag on `MinimizeRoomRank`, replacing
  `MinimizeFirstBlock` / `MinimizeLastBlock` — see
  [ADR-0024](adr/0024-one-type-per-axis-with-flags.md). The two replaced messages
  are **deprecated but retained**, because removing a field is wire-breaking and
  `buf breaking` rejects it. Senders should emit the new type; this repo's
  fixtures were migrated, and the `deprecated` warning is a hard error under the
  lint policy, so a new use cannot land quietly.
* **`PersonPreferenceFit`**, plus a `preferred` field on `Person`. **Both are now
  evaluated** — [ADR-0026](adr/0026-personpreferencefit-charges-the-unmet-fraction.md).
  `None` is the "no stated preference" case, and note the emptiness is INVERTED
  against `Unavailability`: an empty axis there means "every value on that axis",
  an empty `Preference` means no preference at all. The one part still refused is
  a non-empty `PersonPreferenceFit.roles`, which returns `UNIMPLEMENTED` rather
  than widening the counted set beyond lecturers.

  This is also the field whose plumbing was silently incomplete for a while: it
  crossed the wire from `v0.7.0` and the conversion layer dropped it, so the
  app's assembly was write-only against a solver that could not read it.
  `crates/service/tests/person_preference_wire.rs` pins both halves now, because
  each can fail while the other looks healthy.
* **`OnlineOnsiteSameDay` carries a weight**, since it is priced rather than
  forbidden — [ADR-0023](adr/0023-onlineonsitesameday-is-priced-not-forbidden.md).
  A tenant that has not been backfilled sends weight 0, which reads as "count it,
  do not steer" — the same reading every soft type gives a zero weight, and the
  reason the app's rollout order puts the backfill before the deploy.

The verification below was done at `v0.2.0`. The **pipeline** it describes is
unchanged and still the one in force; only the pin moved.

## Verified working at `v0.2.0`, 2026-08-15

The contract repo, this repo's consumption of it, and the contract's CI/CD are all
real and observed working. Only the Nuxt side remains.

### The schema, its tag, and the registry

* Three `.proto` files at `7856748`, tagged **`v0.2.0`** (annotated `a76a203`),
  pushed to `github.com/MindCollaps/calendry-proto`.
* **`@mindcollaps/calendry-proto@0.2.0` is published** to GitHub Packages and
  publicly listed. Verified via the public package page; the registry REST and
  npm endpoints return 401/403 unauthenticated, so *installability* was not
  exercised — that is the Nuxt session's first task.
* `v0.1.0` still exists as a historical marker but was **never published**: the
  enum rename below superseded it before it ever shipped.

### Enum values are prefixed

`WEEK_KIND_*`, `SHARE_WINDOW_*`, `RUN_STATUS_*`. Protobuf scopes enum values to
the enclosing *package*, not to their enum, so bare `EXAM`/`BREAK`/`RUNNING` would
collide with any future enum wanting the same word — a protoc error, not a
warning. Consequences, measured rather than assumed:

* **Rust: unchanged.** prost strips the enum-name prefix, so variants stay
  `Teaching`, `Exam`, … and no solver source needed editing.
* **Binary wire: unchanged.** Only names moved, never numbers.
* **Proto3 JSON: changed**, since JSON uses the value name. Callers must send
  `"WEEK_KIND_TEACHING"`, not `"TEACHING"` — the old value is rejected outright
  (`enum "calendry.solver.v1.WeekKind" does not have value named "TEACHING"`).
  `examples/*.json` were updated accordingly.

### CI/CD in the contract repo, proven in both directions

* `buf.yaml`: `STANDARD` + **`UNARY_RPC`** lint, `FILE` breaking ruleset.
  `UNARY_RPC` mechanically enforces
  [ADR-0005](adr/0005-unary-rpcs-and-solver-owned-run-state.md) — verified it
  rejects a `stream` RPC.
* `validate.yml` on every branch push and PRs to main: `buf lint`,
  `buf breaking` against main, and an independent `protoc` compile. The breaking
  check is **skipped on main by design** (there it would compare main against
  itself); the gate that matters runs on branches and PRs.
* The protoc step carries `if: always()` so a `buf breaking` failure cannot mask a
  protoc failure.
* `publish.yml` on `v*.*.*` tags plus `workflow_dispatch` with `dry_run`
  defaulting to **true**. The git tag is the sole source of truth for the version;
  a dispatch from a branch requires an explicit `version` input and otherwise
  fails fast rather than publishing garbage.
* **Verified by observation, not by reading the YAML:** a throwaway branch
  renumbering `Room.rank` 6→99 produced a red run failing exactly at
  `buf breaking` (`Previously present field "6" ... was deleted.`, exit 100);
  reverting produced a green run where breaking *actually compared* rather than
  skipping; and a combined break-plus-fix run showed the protoc step flip from
  `skipped` to `success`, proving `if: always()`.

### This repo's pin

Submodule `vendor/calendry-proto` pinned to `7856748` = **`v0.2.0`**. The whole
test suite passes against the renamed schema, clippy is clean, and the gRPC path
was re-checked end to end with `grpcurl`.

### Known gap, deliberately accepted

GitHub Actions job logs need an authenticated token, so CI's own tarball listing
could not be read. It was verified instead by reproducing the identical build from
the `v0.2.0` tag: 17 files, 40.4 kB packed / 391.2 kB unpacked, only `dist/**`
plus `package.json` — no `src/generated`, no `node_modules`, no `.proto`.

---

## Still to do — in the Nuxt (`calendry`) repo

**That repo is not checked out here. Do not attempt this from the solver repo.**

* Add `calendry-proto` as a submodule there too, pinned to the same **`v0.7.0`**
  this repo is on.
* Install and import `@mindcollaps/calendry-proto@0.7.0`, and wire the gRPC
  client. Note that only `0.2.0` was ever confirmed published — whether the later
  tags reached GitHub Packages has not been checked from here.
* **Add an `.npmrc` with a GitHub Packages token.** The registry requires
  authentication even to *install* a public package — this hits local dev, the
  docker-compose build, and CI. `calendry` has no `.npmrc` today. This is the one
  part of the pipeline never exercised end to end.
* Add `PersonDoubleBooking` to the app's manual-edit constraint evaluator. See
  below.

### Cross-repo follow-up: `PersonDoubleBooking` in the app

App-repo work, recorded here because this is where the decision was made.

The app's manual-edit evaluator checks room, lecturer and group double-booking,
but **not person double-booking**. Without it a manual edit can create exactly
that clash — a student in two tree-unrelated Groups scheduled at the same slot —
**and the user is never warned**, because "warn and allow" can only warn about
violations its evaluator knows how to detect.

The solver implementing the type does *not* fix this: the solver sees a snapshot
at solve time, whereas manual edits happen continuously between runs. Both
evaluators need the check, and they must agree on its semantics.

### Open dependency: Federation-shared resource occupancy

A tenant's `SolverInput` snapshot needs **occupancy visibility into
Federation-shared Rooms used by other tenants**, so the solver does not
double-book a shared resource across a tenant boundary.

**The mechanism is undecided on the app side** — candidates were a cross-tenant
occupancy ledger versus a narrow database function. **Do not assume a shape for
this and do not design around a guess.** Raise it rather than resolving it
unilaterally. The solver-side requirement is only that the input format can carry
opaque external-occupancy blocks against shared Rooms, which it already does.
