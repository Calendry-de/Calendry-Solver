# Scope membership is carried into `Problem`, not inferred from placement presence

`constraints::exact_frequency` used to treat "this Offering owns at least one
placement variable" as its proxy for "this Offering is in scope". That inference
is lossy in exactly one direction, and it is the direction that matters:
deducting already-locked Sessions can drive an in-scope Offering's placement
count to **zero**, at which point an over-supplied Offering — more locked
Sessions than it requires — was indistinguishable from one nobody asked about,
and its frequency mismatch went unreported.

`Problem` now carries real membership, declared through `ProblemSpec.scope` and
read via `Problem::in_scope`. The conversion boundary already resolved the scope
set, used it twice and then discarded it, so no new information was needed —
only a place to put it.

## Consequences

This changes observable behaviour: an over-supplied in-scope Offering now reports
an `ExactFrequency` violation. The previous silence was itself asserted by a test
so that changing it had to be deliberate; that test now asserts the fix.

`Problem::residual_for` exposes the arithmetic — `required − placements −
immovable` — so the three independent assemblies can be checked against one
definition. It must **expose** over-supply, never reject on it: "warn and allow"
means a caller can legitimately send more Sessions than an Offering claims to
need, and `saturating_sub` keeps that from wrapping a `u32` into four billion
placement variables.
