# Schema distribution, and what remains

Status, not decisions. The decision this records is
[ADR-0003](adr/0003-proto-schema-as-a-pinned-submodule.md) — the schema lives in
a separate repo, consumed as a pinned submodule.

## Current pin: `v0.7.0`

Submodule `vendor/calendry-proto` is pinned to `6107eb2` = **`v0.7.0`**, up from
`v0.2.0`. What arrived between them that this repo cares about:

* **`MinimizeBlockUsage`** and an `invert` flag on `MinimizeRoomRank`, replacing
  `MinimizeFirstBlock` / `MinimizeLastBlock` — see
  [ADR-0024](adr/0024-one-type-per-axis-with-flags.md). The two replaced messages
  are **deprecated but retained**, because removing a field is wire-breaking and
  `buf breaking` rejects it. Senders should emit the new type; this repo's
  fixtures were migrated, and the `deprecated` warning is a hard error under the
  lint policy, so a new use cannot land quietly.
* **`PersonPreferenceFit`**, plus a `preferred` field on `Person`. Neither is
  evaluated: the conversion layer refuses the constraint as `UNIMPLEMENTED`, and
  `preferred` is read by nothing. `None` is the "no stated preference" case.
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
