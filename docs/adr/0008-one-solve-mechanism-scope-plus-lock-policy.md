# One solve mechanism: a scope plus a lock policy

A solve request carries a **scope** — which Offerings are being actively placed —
and a **lock policy** for everything outside it. There are no separate "full
rebuild" and "partial re-solve" modes; a full rebuild is simply a scope covering
everything.

Two modes would mean two code paths with two sets of bugs, and every feature
would have to be implemented twice.

**v1**: everything outside scope is hard-locked and never moved.
**v2**: the hard lock becomes a soft minimize-movement penalty, so the solver
*may* disturb out-of-scope Sessions when genuinely necessary but is biased
against it by `SolveScope.minimize_movement_weight`.

`Immovable` records *why* each piece of occupancy cannot move — `Locked`,
`Past`, `OutOfScope`, `External` — precisely so that v2 is a policy change rather
than a rewrite: it relaxes `OutOfScope` and no other variant.

**In both versions, Sessions whose time has already passed are excluded from
recalculation entirely.** That is a correctness rule, not a tunable preference.

## v2, landed 2026-08-29

Three decisions were not settled by the paragraphs above, and are recorded here
rather than left to be reverse-engineered from the diff:

**The weight is a new wire field, not a hardcoded constant.** Every other soft
term in this codebase (`SoftInstance`, `DayMixInstance`, `PreferenceInstance`)
carries a tenant-configured weight; `LockPolicy` had the enum value but nothing
to weigh it by. `SolveScope.minimize_movement_weight` (`calendry-proto` v0.10.0)
closes that gap rather than picking an internal magic number — the same
cross-repo shape every other tenant-tunable weight already has. Plain `double`,
not `optional`: `0.0` is `LOCK_POLICY_MINIMIZE_MOVEMENT`'s own "track it, do not
steer" reading, the same one every other soft weight already gives a zero, so
there is no "unset vs zero" distinction to preserve.

**A room-only change counts as "moved" too.** The penalty is one knob, not a
slot/room split: `Problem::movement_cost` charges the configured weight once
whenever the placement's `(slot, room)` differs from its `original` in either
component, not only when the slot changes. A Session relocated to a different
room at the same time has still been disturbed.

**`OutOfScope` is relaxed only when the Session realizes a real Offering.** A
`PlacementVar` has nowhere to carry its own occupant data — every other
placement is governed entirely by its Offering's lecturers, groups and eligible
rooms — so an ad-hoc Session (no `offering_id`, e.g. a `staff_meeting` kind) has
nothing for "movable" to mean and stays hard-locked under either policy. This is
a narrower reading of "v2 relaxes `OutOfScope`" than the literal variant name
suggests, and is why `partition_sessions` checks `offering.is_some()` before
routing a Session into the movable path rather than the hard-locked one.

The mechanism otherwise follows exactly what `Immovable` was already shaped for:
`partition_sessions` (`crates/service/src/convert.rs`) routes a relaxed
`OutOfScope` Session into a `PlacementVar` carrying `original: Option<(SlotIdx,
Option<RoomIdx>)>` instead of a `FixedSpec`. `Problem::movement_cost` is a
direct compare against `original` — no precomputed table, unlike
`PersonPreferenceFit`, because the cost depends only on where the Session
already was, not on who leads it. It is wired into the objective exactly where
`preferences.cost` is: `evaluator::score_one`, `Trial::place`/`unplace`,
`ruin_worst`'s attribution and `recompute_objective`, landing in the ordinary
`Objective::soft` field rather than a new one. `search::construct` seeds a
movable Session back at its original placement first, when the original room is
still eligible for its Offering and nothing else occupies it — so the search
does not gratuitously pay a penalty for a move nobody asked for, and falls
through to the ordinary greedy scan (pricing the resulting move) when it
cannot.

## In-scope stay-put pressure, landed — issue #58's measured gap, closed

Issue #58 ("In-scope Sessions have no stay-put pressure") measured, rather than
assumed, that this was worth fixing: scoping a targeted repair to one Offering
churned 36–100% of that Offering's OTHER Sessions across every benchmark preset
(`crates/gen`'s churn report), because `movement_cost` returned `0.0` for every
in-scope placement — there was nothing biasing the search toward the Sessions'
existing slots at all. The card named two candidate shapes and deliberately did
not pick one; this is that pick, and the reasoning for it.

**Chosen: a second, independent weight on the SAME mechanism, not a
session-level scope.** `SolveScope.minimize_inscope_movement_weight`
(`calendry-proto`, `SolveScope` field 5) is `movement_weight`'s in-scope
counterpart — same `original: Option<(SlotIdx, Option<RoomIdx>)>` field on
`PlacementVar`, same `Problem::movement_cost` charge, same wiring into
`evaluator::score_one`/`Trial::place`/`unplace`/`ruin_worst`/
`recompute_objective`. The alternative — scoping by Session id rather than by
Offering — is the "largest wire change" the card itself flagged, and cuts
across `resolve_scope`'s existing "an Offering is in scope if any of its Groups
is" rule; nothing forced that redesign once a second weight sufficed.

**A separate weight, not a shared one, because the two conflate different
magnitudes** — the card's own words, kept: "do not disturb the neighbours"
(out-of-scope) and "do not churn what a targeted repair was not asked to
fix" (in-scope) are different products sharing one mechanism, and a tenant
tuning one must not be silently retuning the other.

**Which weight applies is the placement's Offering's SCOPE, not merely
whether `original` is set** — `Problem::movement_cost` now reads
`self.in_scope(var.offering)` to choose. This is what makes summing both
weights into `hard_penalty` and `initial_temperature` safe rather than
double-counting: a placement's Offering is in scope or is not, so the two
terms never both charge the same placement.

**`partition_sessions` sets `original` unconditionally for every reused
in-scope Session**, not only when the new weight is nonzero. Two reasons: it
is harmless at weight `0.0` (the read-only reading every other soft weight
already gives a zero), and it ALSO seeds `search::construct`'s existing "try
the original first" fast path (see the v2 addendum above) for these Sessions
too — a second, free reduction in gratuitous churn that needed no weight at
all, since that fast path already reads `original` regardless of scope.

Not addressed by this: the churn *measurement* harness itself
(`crates/gen`'s `churn.rs`) still reports the pre-fix numbers — it was built to
confirm the gap was real, not to track this fix's effect, and re-running it
with the new weight configured is future work if the effect size is ever
worth a documented number.

## Per-entity movement overrides, landed (issue #70)

Both weights above are run-wide: one number for everything outside the scope,
one for everything inside it. Issue #70 asked for the axis they cannot express
— a repair-mode selector marking specific People and Groups as fine to move,
or as "please don't unless nothing else resolves this".

**Chosen: a third field on the SAME mechanism.**
`SolveScope.movement_overrides` (`calendry-proto`, `SolveScope` field 6) is a
list of `MovementOverride { oneof target { person_id | group_id }, weight }`,
and an entry covering a placement REPLACES whichever of the two weights above
would otherwise have applied to it. Same `original` field, same
`Problem::movement_cost` charge, same wiring downstream — nothing new on the
objective, and no new term for `hard_penalty` to bound beyond a larger
ceiling.

**One number, not a movable/unmovable flag plus a weight.** The issue names
two settings; replacement makes both fall out of one field. `weight: 0` is
"movable, no extra cost" *even under a large run-wide weight*, which is
precisely what a flag-plus-weight shape would need a special case for, and a
large weight is soft-unmovable. Note what this is NOT: still SOFT, so unlike a
Session `lock` it can never prevent a move. That distinction is the whole
reason the issue exists — a hard lock already existed and was too blunt.

**Replacement, not `max`, and not a multiplier.** `max(base, override)` can
only ever protect more, so "movable, no extra cost" becomes inexpressible the
moment the run-wide weight is nonzero — half the ask, gone. A multiplier makes
`override × 0` and `base == 0` indistinguishable and forces the app to reason
about a product.

**On `SolveScope`, not on `Person`/`Group`, and not a `ConstraintConfig`.** It
is per-RUN policy: the same lecturer is protected in the repair that must not
disturb their week and freely movable in the end-of-term rebuild. Putting it on
the entity would make that a data edit before every run. A constraint type was
never available — ADR-0028 records that a `ConstraintConfig` carries policy and
has never named a specific entity, which is the whole reason
`OfferingRelation` is its own message.

**Scope on each axis follows an existing precedent rather than inventing one:**

- A `person_id` covers Sessions that Person **LECTURES**, never ones they merely
  attend — ADR-0026's decision for `PersonPreferenceFit`, for its reason: an
  attendee set averages ~65 people at benchmark scale, so an attendee reading
  would leave nearly every Session in a real instance overridden. A student's
  protection goes through their Group.
- A `group_id` binds that Group **and its descendants**, so the query walks UP
  — `GroupClosure::expand_ancestry`, exactly as `GroupVeto` does (ADR-0027),
  and deliberately not the both-directions propagation double-booking uses. A
  protected programme protects its cohorts' Sessions; a protected seminar group
  does not protect the lecture its whole cohort attends.
- Several overrides covering one Session: **the LARGEST weight wins.**
  Order-independent, so the answer cannot depend on the order the app sent
  them, and a broader "movable" never silently defeats a narrower protection.

**Resolved once per Offering at build time, into
`Problem::offering_movement_weight`.** An Offering's lecturers, Groups and
scope are all fixed before the search starts, so the answer cannot change
mid-run — which is what lets `movement_cost` keep its `(placement, start,
room)` signature and stay one indexed read on the hottest path in the solver.

**The one approximation, and which direction it errs.** A genuine lecturer POOL
has no fixed lecturer set — that is a search-time choice — so an exact answer
would have to be priced per candidate, the trap ADR-0026 records for the
preference table. Instead an override matching ANY candidate covers the
Offering. That over-protects rather than under-protects, which is the safe
direction for a soft bias: a protected person's Session stays protected
whichever pool member ends up teaching it.

**`hard_penalty` had to grow.** Its bound is "each term costs at most its own
ceiling per placement", and an override replaces the base, so the ceiling
moved. The largest configured override is now folded in alongside both base
weights — looser than necessary, safe for the same reason the other two terms
are, and guarded by a test that a protected Session sitting still cannot
outrank an unplaced one.

**What this does NOT do: it cannot un-lock anything.** Under
`LOCK_POLICY_HARD` an out-of-scope Session is `FixedSpec` occupancy with no
`PlacementVar` at all, so there is nothing for an override to charge.
Movability stays the lock policy's question; an override only prices it.
