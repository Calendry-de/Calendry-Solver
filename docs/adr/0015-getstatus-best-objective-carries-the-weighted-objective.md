# `GetStatus.best_objective` carries the weighted objective, not a violation count

Through slices 1–2 this field carried the **hard-violation count**. Since slice 3
it carries the real weighted objective, `hard × hard_penalty + soft_sum`.
`ObjectiveBreakdown` is also populated; it previously shipped empty.

Recorded because it is a **breaking change for the Nuxt integration**. Nothing
consumes it yet, but anything built against the old meaning must be updated.

The hard penalty is **derived, never tuned**: `sum(weights) × placements + 1`.
Both `unplaced` and `aggregate` sit on the hard side and are covered by the same
bound, so the scalar objective still orders lexicographically without a magic
constant.
