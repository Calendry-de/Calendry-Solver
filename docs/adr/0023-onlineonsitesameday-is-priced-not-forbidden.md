# `OnlineOnsiteSameDay` is priced, not forbidden

It was a feasibility filter inside `SearchState::is_free`, so the search could
never produce a mixed day and `aggregates()` only ever reported one that arrived
in the caller's immovable input. It is now scored on the objective at the
configured weight, so a run may return a schedule that mixes when every
alternative cost more.

`MaxOnlineShare` was already the template: an aggregate that cannot be a filter
and lives on the objective instead. The difference is that a share breach is
charged at `hard_penalty` and a mixed day at its configured weight — that is the
whole of "soft" here.

## Why the term is not in `SoftModel`

That model is a precomputed `(slot, room)` table, exact because the unary soft
types depend on the candidate alone. A mixed day is a property of what **else** is
already placed for the Group that day, so it cannot be read off such a table. It
keeps its own instance list and its own counters — the ones it already had as a
filter.

## Why `Objective.day_mix_cost` is its own field

`soft` is accumulated as a per-placement delta; this is read whole off the
counters, like `aggregate`, because a mixed cell belongs to no single placement.
Folding it into `soft` would mix an accumulated total with an assigned one in one
field, and the debug-only drift assertion that keeps the incremental objective
honest could no longer tell them apart. Stored pre-multiplied so `total()` keeps
its signature.

`Trial::objective` reads it alongside `aggregate` for exactly this reason — see
[ADR-0016](0016-scope-membership-is-carried-not-inferred.md)'s sibling work on
`Trial` owning the objective.

## Why `hard_penalty` multiplies by cells, not placements

`sum(weights) × placements + 1` is the wrong bound for this term in both
directions: one placement can mix several cells at once (it spans days and
implicates its whole subtree of Groups), while two placements are needed before
any cell is mixed at all. The exact ceiling is every cell being mixed, which is
what the counter table is sized for, so that is the multiplier — tight rather
than merely safe.

## A mixed day is no longer a hard violation

`aggregates()` stops emitting them and `soft_breakdown()` carries them instead,
with the cell count and the weighted cost. Listing a priced outcome among hard
violations would report the objective doing its job as a defect.

This is why the report taxonomy is named `ConstraintType` rather than
`ViolationType`: one catalogue type is now reported through the soft-component
channel while the rest go through `Violation`, and the enum names the constraint,
not the channel.

## Consequences

This widens the tracked `ruin_worst` blind spot, reported rather than discovered
later. `ruin_worst` ranks placements by `problem.soft.cost` alone. Among the
tunable terms it used to see 100% and now sees 45.7% — the day-mix term is larger
than the entire pre-existing soft objective and is structurally invisible to it,
since a mixed cell belongs to a `(group, day)` and not to any placement it could
rank. The bench prints that ratio every run so the number moves when the code
does. See [ADR-0025](0025-maxonlineshare-is-not-enforced-by-the-search.md).
