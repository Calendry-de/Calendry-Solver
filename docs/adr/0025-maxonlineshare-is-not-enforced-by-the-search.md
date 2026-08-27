# `MaxOnlineShare` is not enforced by the search, and never was

Fixing the virtual-room bug ([ADR-0022](0022-a-virtual-room-is-not-an-exclusive-resource.md))
more than doubled `MaxOnlineShare` violations at large-university — 180 → 455 —
and that one constraint type is the whole objective regression. Structural
violations were unchanged at exactly 80 and `unplaced` stayed 0 at every preset,
so the fix moved nothing it should not have.

The mechanism: virtual rooms are the **overflow valve** when physical rooms are
full at a slot — they sort last in `eligible_rooms`, so construction reaches them
last. The capacity-1 bug held that valve nearly shut, which incidentally kept
online usage down. `MaxOnlineShare` is deliberately **not** a construction filter
(a ratio whose denominator has not grown yet would dead-end construction — see
the constraint shapes in `CLAUDE.md`), so with the valve open nothing bounds
online usage until LNS.

**The honest framing: this is a pre-existing weakness the fix *revealed*, not one
it caused.** The search was never enforcing that cap; a room-occupancy accident
was. The presets were calibrated in slices 5 and 6 against a solver whose online
capacity was accidentally limited.

## Decisions

Four findings were measured. The numbers live in
[`PERFORMANCE.md`](../PERFORMANCE.md); what is decided here is what to do about
them — and **the first decision is not to act on any of them without
re-measuring**, because [ADR-0021](0021-measure-end-to-end-before-optimizing-a-component.md)
applies and finding 1 is a worked example of exactly why.

**Do not recalibrate the presets to make the numbers look like the pre-fix ones.**
Lowering `max_online_share`, or adding virtual rooms, until the counts resemble
what they were would tune the benchmark to what the solver currently does. That
is the same failure mode as the bug just fixed — a cap enforced by accident
rather than by the search — with the accident relocated from the occupancy layer
into the preset file. **A falling violation count must never be the justification
for a preset change.** If a preset moves, it moves because the instance became
more realistic, and the number is an outcome.

**Fix `ruin_worst`, do not add a fourth ruin arm.** `ruin_worst` is documented as
"the placements contributing the most soft penalty", which was right in slice 3
when soft *was* the objective. Slice 4 moved `unplaced` and `aggregate` onto the
hard side and the operator was never updated, so one third of the search's
selection is aimed at a rounding error. Scoring total objective contribution is a
smaller change, removes an inconsistency rather than working around it, and fixes
the same problem for any future aggregate type. A share-targeting fourth arm
would work and is cheap, but it works around the inconsistency instead of
removing it.

The one piece of genuine design work in that fix: a share breach is a property of
a *group's ratio*, not of any single placement, so "which placement is
responsible" needs a convention — most likely every online placement in a
breaching group. That is the part to think about rather than assume.

**These are complementary, not alternatives.** Budget buys the annealing that
currently never happens without making the search smarter; ruin-selection
correctness is the right long-term fix and stands on its own merits regardless of
this bug. What to resist is recalibrating *instead of* either — the option that
makes the symptom disappear while improving nothing.

## The generator lets every Offering go online, including labs

The generator's eligible-room filter is capacity plus features only, and virtual
rooms are built with `capacity: u32::MAX` and every feature — so every virtual
room is eligible for every Offering, lectures and labs alike. There is no
`allow_online` concept in the generator at all, though the wire and the
conversion layer both have one.

That models an institution where 100% of teaching could be delivered online,
bounded solely by a share rule. No real tenant looks like that; labs are the
obvious counterexample, and the generator already labels them. Recorded as a
realism gap rather than fixed, because changing it moves every preset's numbers
and must be done as a deliberate calibration change, not as a side effect.

## Not this repo: the app's default move budget

Separated from the above because it is live rather than tracked, and because the
change belongs in `calendry`.

The app sends `maxMoves: 50_000` with `maxWallMillis: 10_000` — a quarter of the
bench default that produced every number here, roughly 21 iterations and about
0.35% of a large instance repaired. The move budget binds long before the wall
budget, so around 9.7 of the 10 granted seconds go unused, and 5M moves (2.28 s
at the largest preset) would fit inside the existing allowance with room to
spare.

Raising `max_moves` is also the **determinism-safe** axis: only move-budget
termination is reproducible ([ADR-0006](0006-two-budgets-and-the-limit-of-determinism.md)),
so spending the budget there preserves the guarantee while leaning on the wall
clock destroys it.
