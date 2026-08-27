# Hybrid constructive heuristic plus local search, CPU-only for v1

Timetabling here is a constraint **optimization** problem: hard constraints
define feasibility, soft constraints define a weighted objective. The prototype
(TimeCraft, a prior student project) used CP-SAT via OR-Tools, which finds
provably optimal answers but scales badly to a 27,000-Session university and
gives nothing useful when interrupted.

We use a greedy constructive heuristic followed by Large Neighborhood Search
with simulated-annealing acceptance. It has a usable answer at every point in
the run, which is what a time-boxed interactive service needs, and it degrades
gracefully on infeasible input — which the app's "warn and allow" editing UX
routinely produces.

CPU-only, using `rayon` for data parallelism. See ADR-0013 for the GPU seam.

## Consequences

The solver returns a good answer, never a provably optimal one. Anything that
wants optimality proofs needs a different engine.
