# A rotating block pattern decomposes into three parts, two of which already exist

`#69` on the Calendry board asks for "a fixed weekly rotation for a stretch of
the term: *every Monday: English, Math, German* (in that block order),
repeating for, say, 5 weeks before the next rotation or a different pattern
kicks in", adjustable on how many blocks make up the pattern, which
Offerings/kinds occupy which block and day, and how many weeks it repeats. The
card filed itself explicitly unsized, with three open questions and a note that
it "needs a short design conversation before it's sized" — and that the repo
might be `calendry` rather than `calendry-solver` depending on the answer.

This ADR is that conversation. **It concludes with no new constraint type and
no wire change**, which is the point rather than a disappointment: the ask is
not one feature, it is three, and two of the three are already built.

## First: which of two readings is being asked for

"Which Offerings occupy which block/day" is doing a lot of quiet work in the
card, because it can mean two very different things:

* **Fully specified** — the tenant states *English at Monday block 0, Math at
  block 1, German at block 2*. That is not a constraint on a search. It is a
  complete answer: fifteen placements, over five weeks, with nothing left to
  decide. A constraint type that "enforces" it would be scoring a solution the
  tenant has already written.
* **Shape only** — *these three Offerings occupy Monday's three blocks in some
  order the solver picks, and whatever order it picks holds for five weeks*.
  That genuinely constrains without determining.

The card's phrasing ("which Offerings/kinds occupy which block/day" as a thing
the tenant adjusts) reads as the **fully specified** one. Its own framing
already noticed the consequence: *"'build a whole recurring weekly template' is
closer to authoring input than scoring output, so it may not fit the
`ConstraintType` shape at all."* That instinct is correct, and it is the first
finding here.

**A fully specified rotation needs no solver work whatsoever.** The app writes
those Sessions and locks them; the solver has respected locked Sessions since
v1 ([ADR-0008](0008-one-solve-mechanism-scope-plus-lock-policy.md)) and will
schedule everything else around them. Adding a constraint type for this would
be building a second, weaker mechanism for something a lock already does
exactly.

So the shape-only reading is the only one that could justify solver work, and
the rest of this ADR is about that.

## The rotation is not one rule; it is a within-week half and an across-week half

Separating those two is what makes the ask tractable, because they have
completely different answers.

### The within-week half is already expressible

*"English, then Math, then German, in Monday's three blocks"* — as a shape
rather than an assignment — is a set of ordinary pairwise rules:

| Wanted | Already built |
|---|---|
| All three on the same weekday | `SameDays` relation ([ADR-0028](0028-a-relation-is-an-ordered-set-of-offerings.md)) |
| In that block order | `Precedence` relation, which reads member order and is the only kind that does |
| Never colliding | The four structural double-booking types, which the search cannot violate |
| Adjacent rather than scattered across the day | `Compactness`, `MaxConsecutiveBlocks` |

Nothing here is missing, and nothing about it is specific to rotation. This is
the payoff of `OfferingRelation` being one mechanism rather than one message
per rule: a requirement that reads as novel turns out to be a combination.

### The across-week half is ALSO already built — as `DistributedPatternAdherence`

This is the finding that changes the sizing. `Offering.scheduling_pattern =
SCHEDULING_PATTERN_DISTRIBUTED`, priced by `DistributedPatternAdherence`,
already does exactly "a weekly repeating template":

> Prices an Offering tagged `SCHEDULING_PATTERN_DISTRIBUTED` for spreading its
> Sessions across more than one weekly `(weekday, block)` slot. Cost: distinct
> weekly slots occupied, minus one.

Its cost is zero precisely when every Session of the Offering lands on the SAME
`(weekday, block)` — i.e. when the Offering repeats weekly on a fixed slot.
`Aggregates::add_distributed`/`distributed_cost` key on
`SlotTable::weekly_cell`, which is that `(weekday, block)` pair. A rotation
built from Offerings tagged DISTRIBUTED already holds its weekly shape today,
with no new type at all.

## What IS missing: the WINDOW, and it is a parameter rather than a type

`DistributedPatternAdherence` is whole-**term**. The card's "repeating for, say,
5 weeks before the next rotation or a different pattern kicks in" is the one
part it cannot express: a deliberate pattern CHANGE at week 6 is charged as
drift, indistinguishable from an Offering that simply failed to hold its slot.
The tenant is penalized for the very thing they configured.

So if #69 is pursued in this repo, the shape is:

**A window on the existing type, not a new type.** Either a repeated
`{first_week, last_week}` list on `DistributedPatternAdherence`, or a `phases`
notion on the Academic Calendar that every week-spanning aggregate could read.
The cost function itself is unchanged: distinct weekly slots minus one,
evaluated per window instead of once over the term, summed. That is a small
change to `Aggregates`' existing accumulator, and it would also make
`BlockPatternAdherence` and `MinimizeWeekdayImbalance` window-aware for free —
which is an argument for the calendar-phase version over a field on one type,
and a reason not to decide it until a second type actually wants it.

**Deliberately not decided here**, because nothing forces it yet and
[ADR-0021](0021-measure-end-to-end-before-optimizing-a-component.md)'s stance
applies to features as much as to performance: the app has not yet stated which
of the two readings it needs, and the fully specified one — the more likely
one, on the card's own wording — needs none of this.

## Answering the card's three open questions directly

1. **Constraint, or generation-time template?** Neither, as posed. The
   *assignment* is authoring input (locked Sessions, already supported). The
   *shape* is a combination of existing constraints. Only the *window* is a
   constraint-shaped gap, and it is a parameter on a type that already exists.
2. **Per-Offering, or per-Group/tenant?** **Per-Offering**, and the question
   dissolves once the halves are separated. What makes three Offerings a
   "rotation" is not a set-level object: it is each Offering individually
   holding its weekly slot, plus the ordinary pairwise rules keeping them
   ordered and non-colliding. A set-level template would be a fourth way to
   name a group of Offerings, competing with both `applies_to_kinds` and
   `OfferingRelation` — the multiplication ADR-0028 exists to prevent.
3. **Interaction with TimeGrid breaks and the term calendar?** Already correct,
   and worth recording so nobody "fixes" it. A break or holiday week inside a
   five-week run simply has no Session in it, and "distinct weekly slots minus
   one" counts slots rather than weeks — so a closure costs nothing and shifts
   nothing. A rotation spanning a break behaves the way a reader would expect
   without any calendar-specific code.

## One interpretation flagged, because it would change the answer

"Rotating" is read here as *this fixed weekly pattern holds for N weeks, then a
different one may replace it* — which is what the card's example describes (the
block order is fixed for the five weeks). It is NOT read as a pattern that
cycles WITHIN the run: week 1 English/Math/German, week 2 Math/German/English.

That second reading is a genuinely different rule, and none of the above covers
it: it wants each Offering to visit each slot in turn, which is neither "hold
one weekly slot" nor any pairwise relation. If that is what a tenant means, it
needs its own card and its own design — do not stretch this one to cover it.

## Consequences

* **No `calendry-proto` change and no `calendry-solver` change** for #69 as
  written. The card should be updated to say which reading is wanted, and to
  point at locks and at `SCHEDULING_PATTERN_DISTRIBUTED` for the parts that
  already work.
* **`DistributedPatternAdherence` is more capable than its name suggests**, and
  that is now written down. "Distributed" describes the tenant-facing pattern
  tag; the cost function is "hold one weekly `(weekday, block)`", which is the
  weekly-template primitive. A future reader looking for a repeating-template
  feature will not find it by searching for "rotation" or "template".
* **The window gap is real but shared.** If it is built, build it where
  `BlockPatternAdherence` and `MinimizeWeekdayImbalance` can use it too, rather
  than as a field on one type.
