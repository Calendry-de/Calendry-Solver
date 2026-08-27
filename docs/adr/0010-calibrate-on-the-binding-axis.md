# Benchmark instances are calibrated on the binding axis, not on room tightness

The obvious difficulty measure is `demand_blocks / (rooms × slots)`. It is the
wrong one, and calibrating against it produced instances construction could not
solve at all — measured: 565 of 3,968 Sessions placed.

The reason is conflict propagation. A Room's row accumulates only the Sessions
placed in that Room. A **Group's** row accumulates every Session of every Group
in its conflict closure, so a Cohort is marked busy by its entire subtree.
Demand that spreads across many Rooms piles onto a single Cohort row.

The generator therefore calibrates `saturation`, the maximum over the room,
Group, Lecturer and person-clique axes, into a 0.55..=0.75 band. At realistic
hierarchies the Group axis binds in every preset and room tightness sits at
0.24–0.38 — that is correct, not a defect.

Cohorts are assigned **round-robin, not at random**: because the cohort row
binds, random assignment lets the *busiest* cohort decide feasibility, and the
maximum of N draws grows with N. Measured saturation overshot the closed form by
1.28x at school scale and 1.55x at university scale, so no single calibration
held across the range. Class and seminar choice within a cohort stays random.

## Consequences

`InstanceStats` carries both the closed-form prediction and the measurement, and
`prediction_error` asserts they **agree** — asserting only that each lands in the
band lets a badly wrong model pass green.
