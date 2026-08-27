# A load metric cannot certify feasibility: the person-clique bound

Group, Lecturer and Room load all ask *"how busy is one row"*. There is a whole
class of infeasibility they structurally cannot see, and it shipped: four
benchmark presets were **provably unplaceable** while every axis read "in band".

Two Offerings sharing even one attendee can never occupy the same slot under
`PersonDoubleBooking`. A set that **pairwise** conflicts therefore needs one
distinct slot per Session. Every individual can be lightly loaded while the
attendee *sets* pairwise intersect — that is a graph-colouring bound, not a load
bound.

`InstanceStats` therefore carries `person_clique_load`: a greedy maximum-clique
lower bound over the Offering conflict graph, times its Sessions' block demand,
over the term. It is part of `saturation`, and the preset calibration test
asserts it stays under 1.0.

## Consequences

**Above 1.0 is a certificate of infeasibility. Below 1.0 is not a proof of
feasibility** — greedy understates the true clique, so the bound can miss
infeasibility but never invent it.
