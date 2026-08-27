# One solve mechanism: a scope plus a lock policy

A solve request carries a **scope** — which Offerings are being actively placed —
and a **lock policy** for everything outside it. There are no separate "full
rebuild" and "partial re-solve" modes; a full rebuild is simply a scope covering
everything.

Two modes would mean two code paths with two sets of bugs, and every feature
would have to be implemented twice.

**v1**: everything outside scope is hard-locked and never moved.
**v2 (deferred)**: the hard lock becomes a soft minimize-movement penalty, so
the solver *may* disturb out-of-scope Sessions when genuinely necessary but is
heavily biased against it. `LOCK_POLICY_MINIMIZE_MOVEMENT` currently returns
`UNIMPLEMENTED`.

`Immovable` records *why* each piece of occupancy cannot move — `Locked`,
`Past`, `OutOfScope`, `External` — precisely so that v2 is a policy change rather
than a rewrite: it relaxes `OutOfScope` and no other variant.

**In both versions, Sessions whose time has already passed are excluded from
recalculation entirely.** That is a correctness rule, not a tunable preference.
