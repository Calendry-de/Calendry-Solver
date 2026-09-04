# Schema distribution, and what remains

Status, not decisions. The decision this records is
[ADR-0003](adr/0003-proto-schema-as-a-pinned-submodule.md) — the schema lives in
a separate repo, consumed as a pinned submodule.

## Current pin: `c989038` = `v0.19.0`

Checked 2026-09-04 directly against the submodule, not against this file:
`git tag --contains HEAD` in `vendor/calendry-proto` answers `v0.19.0`, and
`v0.19.0` is the newest tag in the contract repo, so there is nothing to bump.
This header had said `v0.10.0` for nine releases while the per-field entries
below were kept current — the same drift the `v0.8.0` correction further down
records, one section up.

Two things changed around the pin rather than in it:

* **The org moved.** The contract repo is `github.com/Calendry-de/Calendry-Proto`
  (`v0.15.1`, "Calendry.de umzug"), and the npm package is
  **`@calendry-de/calendry-proto`**, not `@mindcollaps/calendry-proto`. Every
  `MindCollaps` URL in the sections below was true when written.
* **Every tag from `v0.11.0` to `v0.19.0` published for real.** `publish.yml`
  runs on the tag pushes all completed `success` (`gh run list --workflow
  publish.yml` in the contract repo), so the app can install any of them.

What arrived between `v0.10.0` and `v0.19.0`, and where each is recorded.
Solver-side, **all of it is built**; the catalogue has no `UNIMPLEMENTED`
type left, only the one refused `PersonPreferenceFit.roles` parameter.

| Tag | What it carried | Solver record |
|---|---|---|
| `v0.11.0` | P2 batch: 14 constraint types (room fit, day/week aggregates, tenant policy); `DistributedPatternAdherence` / `BlockPatternAdherence` reading `Offering.scheduling_pattern`; `required_room_count` / `room_ids` and `room_feature_requirements` marked Built | `CLAUDE.md`, [ADR-0024](adr/0024-one-type-per-axis-with-flags.md) |
| `v0.12.0` | `OfferingRelation` + `DifferentTime`; `SolveScope.minimize_inscope_movement_weight` (issue #58) | [ADR-0028](adr/0028-a-relation-is-an-ordered-set-of-offerings.md), [ADR-0008](adr/0008-one-solve-mechanism-scope-plus-lock-policy.md) |
| `v0.13.0` | `MaxDailySessionCount`; the `(Offering, day)` cluster (`MaxOfferingSessionsPerDay`, `MaxConsecutiveOfferingBlocks`, `MinimizeOfferingDaySplit`) | ADR-0024 |
| `v0.14.0` = `v0.15.0` | P0 batch: `TimeGrid.breaks`, `Offering.prefer_fuller_days` + `MinimizeOfferingDistinctDays`, `Room.site` + `TravelTimeBetweenRooms`, day caps, `SameTime` / `SameDays` / `SameStart` | ADR-0028 |
| `v0.15.1` | Org move only, no schema change | — |
| `v0.16.0` | `Precedence` relation; `SolveScope.movement_overrides` (Calendry #70) | ADR-0028, ADR-0008 |
| `v0.16.1` | `SolverOutput.unplaced_offerings` (Calendry #119, proto PR #1); `SolverOutput.retained_session_ids`; `MinimizeSpecializedRoomUse` (Calendry #121). NOTE: this tag is NOT an ancestor of `main` — the `retained_session_ids` commit was re-applied on `main` after the PR merge — so `git describe` on the pin skips it. Its schema content is byte-identical to `v0.17.0` minus `footprint_tags`, verified by `git diff v0.16.1 v0.17.0 -- proto` | [ADR-0031](adr/0031-convergence-is-never-declared-over-unplaced-demand.md), [ADR-0032](adr/0032-the-answer-accounts-for-every-session-it-was-given.md), ADR-0024 addendum |
| `v0.17.0` | `Room.footprint_tags` (Calendry #122) | [ADR-0022](adr/0022-a-virtual-room-is-not-an-exclusive-resource.md) |
| `v0.18.0` | `Week.exam_group_ids` (Calendry #126) | [ADR-0033](adr/0033-an-exam-week-is-scoped-on-the-calendar-and-charged-per-offering.md) |
| `v0.19.0` | `Person.allowed_room_ids` + `LecturerRoomPin` (Calendry #124); `Precedence.min_days_between` (Calendry #55) | [ADR-0034](adr/0034-a-room-pin-is-checked-against-the-candidate-not-precomputed-into-the-offering.md), [ADR-0035](adr/0035-room-sharing-is-a-property-of-the-room.md) |

Several of those fields have their own entries in the `v0.9.0` section
below, because that is where they were first staged PROTO ONLY and the entry
was updated in place when the evaluator landed. Each such entry names its own
pin. They were left where they are rather than moved, so the history of
"staged first, built later" stays readable.

The one field on the wire that is still not consumed is
`SolverOutput.candidates`, and it is marked DRAFT in the proto itself — see
its entry below.

## Earlier pin: `03aed98` = `v0.10.0`

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

## Earlier pin: `855c145` = `v0.9.0`

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

* **`Room.footprint_tags`** (field 13, pin v0.17.0). An open-vocabulary tag
  naming a physical space several Room identities describe — movable walls,
  where booking 1.0 must make 1.1, 1.2 and the Audimax unbookable for that
  slot. Symmetric by construction rather than a directed "A also books B"
  reference, so the two directions cannot be built with one missing; a Room
  may carry several tags, and a tag only one Room carries is inert.
  Solver-side: **done** — resolved into `Problem::footprint_siblings` and
  tested in `Occupancy::is_free`, reported under `RoomDoubleBooking`. A tag on
  a virtual Room is REFUSED (`ConvertError::FootprintOnVirtualRoom`), since a
  virtual Room's occupancy row is never consulted and the tag could only be
  inert. App-side: **done** (Calendry #122, closed) — `room.footprint_tags`
  with a CHECK refusing the tag on a virtual Room, mirroring this repo's
  refusal, and `toWireRoom` sends the tags verbatim. "Room exclusivity groups
  — movable-wall configs".

* **`Room.feature_quantities`** and **`Offering.room_feature_requirements`**
  (with `RoomFeatureQuantity` / `RoomFeatureRequirement`). Today's
  `feature_tags` / `required_room_features` are presence-only, so "needs 24
  workstations" degrades to "needs a workstation". The new fields carry a
  count on both the supply (Room) and the demand (Offering) side;
  `min_quantity` is `optional` for the same zero-vs-absent reason
  `Preference.weight_multiplier` is. **Solver-side: done** (marked Built in
  the contract at `v0.11.0`) — `crates/service/src/convert.rs` checks
  `room_feature_requirements_met` against `Room.feature_quantities` as part
  of per-Room eligibility, so "needs 24 workstations" excludes the room with
  12. "Equipment quantity cannot cross the wire" (board: Done).

* **`Session.room_ids`** and **`PlacedSession.room_ids`**, plus
  `Offering.required_room_count`. `room_id` (singular) remains the primary
  Room and is unchanged for a single-room Session; the plural field carries
  the full set, `room_id` included, only when more than one Room is occupied
  simultaneously. **Solver-side: done** (marked Built in the contract at
  `v0.11.0`) — `convert.rs` enumerates `room_combinations` for
  `required_room_count > 1` (refused above `MAX_ROOMS_PER_SESSION`),
  construction and repair choose among them the same way they choose among
  lecturer combinations, and every Room of the set must pass a Person's room
  pin (ADR-0034). "A Session with more than one Room cannot cross the wire"
  (Calendry #10, #59; both Done).

* **`Offering.scheduling_pattern`** (`SchedulingPattern`: distributed vs.
  block/intensive). Staged as metadata; **now read** — the enforcement shape
  resolved to one constraint type per pattern, `DistributedPatternAdherence`
  and `BlockPatternAdherence` (contract `v0.11.0`), each a per-Offering
  aggregate priced only for Offerings tagged with its pattern. `DISTRIBUTED`
  costs "distinct weekly `(weekday, block)` slots, minus one", which is also
  the weekly-template primitive [ADR-0030](adr/0030-a-rotating-block-pattern-decomposes-into-parts-that-already-exist.md)
  points at. "Scheduling pattern per Offering" (Calendry #1, Done). The
  lecturer-facing control surface is Calendry #28, app-side; its third mode,
  "multiple in a day", is `Offering.prefer_fuller_days` +
  `MinimizeOfferingDistinctDays` (`v0.14.0`), already built here.

* **`MinimizeExamWeek.invert`**. One flag, not a new type — the same
  `MinimizeRoomRank.invert` / `MinimizeBlockUsage` precedent. `false` (absent)
  is today's only prior behavior, unchanged; `true` pushes Sessions toward the
  exam period instead of away from it. **Solver-side: done**, 2026-08-29 —
  the instance carries `invert` and
  `crates/service/src/convert.rs` reads `p.invert` from the wire; the
  direction-flip is covered by a falsification test
  (`inverted_minimize_exam_week_steers_into_the_exam_week`, on a grid built
  specifically so the unweighted default sits OUTSIDE the exam week, or a
  dropped `invert` would pass by never having to move). "Exam-specific
  placement logic" (the wire half only; the lecturer-facing "create my own
  exam" flow, and pushing toward TERM-END specifically rather than just
  "the exam period", are still app-side / unbuilt).

  Superseded in one detail as of ADR-0033: the instance is no longer a
  `SoftParams::MinimizeExamWeek` variant. Once `Week.exam_group_ids` let an
  exam week belong to some cohorts and not others, the predicate had to read
  the Offering, so the type moved to `ConstraintSet::minimize_exam_week` and
  `Problem::exam_week_cost`. `invert` and the falsification test above are
  unchanged, and both directions still read one per-Offering mask.

* **`Person.allowed_room_ids`** (field 6, `repeated string`) and
  **`LecturerRoomPin`** (`oneof params` entry 58). Rooms a Person may teach
  in; empty means any Room the Offering allows, which is every Person before
  the field existed. **Solver-side: done** — HARD and a FILTER, checked
  against the placement's CHOSEN lecturers rather than precomputed into the
  Offering, which is what makes it work for a genuine lecturer pool — where
  `LecturerVeto` had to be refused until Calendry #131 gave it the same
  shape. See
  [ADR-0034](adr/0034-a-room-pin-is-checked-against-the-candidate-not-precomputed-into-the-offering.md).
  One refusal (an unknown Room id, since dropping it widens a whitelist);
  three deliberate non-refusals (a virtual Room is honoured, a lecturer pool
  is accepted, an empty list is inert). Calendry #124 v2; v1 is app-only.

* **`Week.exam_group_ids`** (field 4, `repeated string`). Which Groups a week
  is an EXAM week FOR; empty means every Group, so every peer on an earlier
  pin keeps today's term-global behaviour exactly. **Solver-side: done** —
  see [ADR-0033](adr/0033-an-exam-week-is-scoped-on-the-calendar-and-charged-per-offering.md)
  for why the scope narrows `Week` rather than adding a second message, and
  why the query walks UP through `expand_ancestry`. Two refusals, both because
  inert would silently widen: an unknown group id, and a non-empty list on a
  week that is not an exam week (`ConvertError::ExamGroupsOnNonExamWeek`).
  Calendry #126 sub-ask 3; sub-asks 1 and 2 are app-only.

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
  accepts. **Solver-side: done** — corrected 2026-09-03, having said "not
  started" for several releases after it shipped. `PreferenceModel::build`
  carries `room_features` as its own axis FAMILY, and it is priced on both
  paths: the precomputed per-placement table and, for a genuine lecturer
  pool, the live per-person one, through
  `Problem::preference_cost_for_placement`. "Room-type preference kind".

  The staleness mattered rather than being untidy: while this sentence stood,
  a soft person-to-room constraint looked like an open option instead of a
  duplicate of a shipped one — see
  [ADR-0034](adr/0034-a-room-pin-is-checked-against-the-candidate-not-precomputed-into-the-offering.md),
  which had to establish this before it could rule the soft variant out.

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

The integration this section once listed as unbuilt exists: the board's
app-side comments (Calendry #22, #24, #119, #122) describe `solverInput.ts`
assembling a full `SolverInput` snapshot per Generation, a gRPC client driving
`StartRun` / `GetStatus`, and a materialization step applying the result. What
this repo still cannot verify from here, and should not assume:

* **Which pin the app is on.** Every tag through `v0.19.0` is published as
  `@calendry-de/calendry-proto`; whether the app consumes `v0.19.0` is a
  question for that repo. The fields the board still lists as awaiting the app
  are `SolverOutput.unplaced_offerings` (#119), `Person.allowed_room_ids`
  (#124), `Week.exam_group_ids` (#126), `SolveScope.movement_overrides` (#70),
  `MeetTogether` (#55) and a banked Session sent with `start_slot` unset (#22).
  ADR-0033 records why the exam-week field must be gated on the pin rather
  than treated as benign when omitted.
* **The `.npmrc` / GitHub Packages install path.** The registry requires
  authentication even to *install* a public package. Whether local dev, the
  docker-compose build and CI all carry a token was never checked from here.
* `PersonDoubleBooking` in the app's manual-edit constraint evaluator. See
  below; not confirmed either way.

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
