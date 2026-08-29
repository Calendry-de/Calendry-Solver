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
