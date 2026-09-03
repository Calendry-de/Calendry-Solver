# A distribution relation is an ordered set of Offerings plus a type

Every constraint today scopes itself with `applies_to_kinds` — it names a
**category, never a row**. That holds even where it looks otherwise:
`LecturerVeto` names no lecturer, because the windows live on `Person.blackouts`
and the config only switches enforcement on; `GroupVeto` is the same shape.
**A `ConstraintConfig` carries policy. It has never carried a reference to a
specific entity, and no field in the message could hold one.**

A whole family of requested rules needs exactly that: *"Offering `db-lecture` and
Offering `db-lab`"*. `Precedence`, `DifferentTime`, `SameTime`/`SameDays`/
`SameStart`, `MeetTogether`/`CanShareRoom`, and the ten-odd "N hours between"
variants are all blocked on the same missing pointer.

**They get one mechanism, not one per type.** A distribution relation is an
**ordered set of Offering references**, a **relation type**, and that type's
parameters. Only the evaluator differs between relations; the reference, its
storage, its form control, its dangling-reference semantics and its scale
behaviour are built once here or five-to-fifteen times downstream.

UniTime is the evidence rather than the inspiration. It carries roughly forty
distribution preference types on exactly this shape — a set of classes plus a
type string plus a preference level — and that is why a system that old keeps
absorbing new requirements cheaply. **Its forty constraints are forty values of
one field, not forty features.**

## The set is ORDERED, and three types in forty are why

Order looks like dead weight: `DifferentTime`, `SameTime`, `MeetTogether`,
back-to-back and every "N hours between" variant are symmetric, and for those a
set is a set. UniTime's catalogue is the counter-example, and it is a small,
specific one — **exactly three of its types read the order**: `Precedence` ("the
first meeting of the first class has to end before the first meeting of the
second"), `Next Day`, and `Two Days After`.

Three in forty is enough, because the alternative is not "no ordering". It is a
second reference mechanism for the ordered relations, which is the multiplication
this ADR exists to prevent — and `Ordering between Offerings` (a lab must follow
its lecture) is one of the five cards waiting on this one.

Position is one integer on the membership row. Adding it later is worse than it
sounds: it would leave every existing relation order-ambiguous, so the first
ordered type would have to invent an order for rows that never had one, or refuse
them. A symmetric type simply ignores the field.

## An N-ary set, not a pair

`MeetTogether` and choice blocks — three electives timetabled against each other
so a student picks exactly one — are naturally three-way and upward. A pair type
cannot grow into a set without a schema break, while a set of two costs a
symmetric type nothing.

**No grouping dimension in v1.** UniTime additionally lets a preference say how
the set is carved up: `All Classes`, `Pairwise` (expanding to n(n−1)/2
constraints), `Groups of Two`, `Progressive`, `One Of Each`. That is a real
generalisation and none of the five waiting cards needs it. The set IS the
constraint here; a `structure` field defaulting to "the whole set" is purely
additive if one is ever wanted.

## Membership is a NEW carrier, never `ConstraintScope`

`constraint_scope` in the app already has an `offering_id` with a real foreign
key and a cascade — dormant, because `toWireConstraint` skips any row carrying
one rather than degrading it to unscoped, which would silently widen the rule to
every Offering. It is the obvious place to put relation members and it is the
wrong one.

**Scope and membership are opposite meanings that would share a table.** For
`MinimizeExamWeek` scoped to Offering X, X is a *filter* — the rule applies to
X's Sessions. For `DifferentTime` over {A, B}, A and B are *operands* — the rule
is about the relationship between them, and applying it "to A" alone is
meaningless. Row existence would mean one thing or the other depending on the
constraint's type, which is the trap `group_term` versus `group_term_availability`
already records: the second table exists precisely because overloading the first
would have made one row's existence carry two incompatible claims.

## What a relation relates is declared per type, never inferred

An Offering is a recurring definition; a relation between two of them has to say
something about their **occurrences**, and the answer is not the same for every
type. UniTime's `Precedence` relates *first meetings*. `DifferentTime` has to
hold for every pair of Sessions. A "next day" relation pairs occurrence *i* of
one with occurrence *i* of the other, which is only meaningful when the two
frequencies match.

Each type declares its own occurrence pairing in its evaluator, the same way
hard-versus-soft is a property of the type rather than a configuration field
([ADR-0007](0007-fourteen-typed-constraint-types-no-dsl.md)). There is no global
default, because a wrong global default is invisible: a relation would quietly
constrain the wrong pairs and still report as satisfied.

## Consequences

**It breaks the locality every fast path assumes.** `score_one` prices a
candidate from the move's own span, the occupancy bitsets and aggregate counters
— all of which describe *this* placement. A relation's cost depends on where a
*different* Offering sits, so:

- **LNS ruin removes both sides.** A relation between A and B where both are
  ruined has no defined cost until both are repaired, and repair is greedy and
  sequential. The first side is priced against a partner that is not placed.
- **`ruin_worst` attribution has no natural home.** A relation's cost belongs to
  a pair, not to a placement — the same problem the aggregates had, and
  `Solution::aggregate_ruin_score` is the shape of the answer: attribute the
  relation's contribution to each member.
- **The O(k) journal reversal assumes a placement's cost is recomputable from
  its own state.** It no longer is.

None of that is a reason not to build it. It is the reason this is a mechanism
ADR rather than a constraint type, and the reason the evaluator work is larger
than the four evaluators it unblocks.

**Scale changes shape.** Every existing instance covers every kind it names, so a
tenant has a handful of constraint rows. Relations are one row per relation:
fifty related pairs is fifty rows, and both the app's constraint builder and its
list view are built for a handful.

**A dangling member is a typed `ConvertError`, not a skip.** An Offering absent
from the snapshot is not "occupancy either way" the way a Session naming an
unknown Offering is ([`partition_sessions`](../../crates/service/src/convert.rs)) —
a relation with one side missing is a rule that cannot be evaluated, and running
it half-applied would satisfy it by construction.

**`Compactness` and `PatternAdherence` are not precedents for this, and
`PatternAdherence` is the sharpest illustration of why.** It is about one
specific Offering's pattern, which sounds like a per-row reference — and
`PatternAdherenceInstance` carries `kinds` and a weight, nothing else. The
per-Offering half comes from `Offering.scheduling_pattern`, a field **on the
entity**, exactly the way `LecturerVeto` reads `Person.blackouts`. Both
constraints span sets, and both derive those sets from data the solver already
indexes.

A relation is the first constraint whose operands are *chosen by the tenant in
the constraint itself*. That is what makes the reference, rather than the
evaluator, the hard part — and it is why "put the data on the entity" cannot
rescue this one: a relation is not a property of either Offering, it is a
property of the pair.

## `Precedence` landed (issue #37) — the order paid for itself, once

The section above bet that "three types in forty" justified an ordered set.
`Precedence` is the first of the three to be built, and it is the only kind on
the mechanism that reads `members`' order at all — every other built kind
(`DifferentTime`, `SameTime`, `SameDays`, `SameStart`, `MeetTogether`) is
symmetric and ignores it. The bet was cheap and it paid: the order was already
there, `build_relations` never normalized it, and the evaluator is a
`members.windows(2)` walk. Had position needed adding here, every relation
already configured would have been order-ambiguous, exactly as predicted.

**Its occurrence pairing is TERM-WIDE, ALL PAIRS**, which this ADR required
each type to declare rather than inherit. Every placed Session of the
predecessor must end before every placed Session of the successor begins — the
block-teaching reading. Two alternatives were live and both were rejected:

- **A per-week pairing** (what the `SameTime` family uses) says nothing about
  a lab in week 2 preceding a lecture in week 3, so the rule a tenant thinks
  they configured holds only inside each week.
- **`UniTime`'s first-meetings-only** ("the first meeting of the first class
  has to end before the first meeting of the second") leaves every occurrence
  after the first unconstrained.

All-pairs also turns out to be the *cheapest* of the three to evaluate, not
the most expensive: "every pair ordered" is exactly "the predecessor's latest
end precedes the successor's earliest start", so each consecutive member pair
has ONE boundary rather than n×m comparisons.

**It is HARD but PRICED at `hard_penalty`**, joining `SameTime`/`SameDays`/
`SameStart` and `MaxDays` rather than `DifferentTime`/`MeetTogether`'s
occupancy filters, and for the same reason spelled out for the `SameTime`
family: the boundary is a property of two Offerings' COMPLETE placed sets, and
no moment mid-construction has both. A candidate filter would have to refuse
on partial information or never refuse a genuine breach. So a run can succeed
while reporting a `PrecedenceRelation` violation — the `ExactFrequency` shape,
not a new exception. Issue #37 recommended SOFT as the safer default; HARD was
chosen instead, which is the same stance every other built relation kind
takes, and the pricing is what keeps it from dead-ending anything.

### Two units, deliberately, and neither substitutes for the other

`Precedence` is the first relation to carry parameters at all
(`min_gap_minutes`, `max_days_between`), and they measure the same boundary in
different units:

- **Ordering is decided structurally**, on a block ordinal
  (`calendar_day * blocks_per_day + block`). It has to be: a caller sending no
  wall-clock structure produces `GridTime::default()`, whose
  `block_length_minutes` is zero, and every minute-of-day on a day then
  collapses to that day's start. Ordering must stay exact there.
- **The gap is wall-clock minutes**, resolved through `GridTime` — block
  lengths, the default gap, every named break. "At least a day between the
  lecture and the lab" is not expressible in block indices.
- **The ceiling is CALENDAR days** (`week * 7 + iso_weekday - 1`), not
  `SlotFlags::day_index`, which is dense over *teaching* days. A tenant saying
  "within 2 days" means the student's calendar; counting teaching days would
  silently stretch that across every weekend and closure week. Note this
  differs from `Daybreak`, which treats consecutive `day_index` values as
  adjacent nights on purpose — that type is about *adjacency*, this one
  *measures a multi-day distance*.

### An open divergence this created: locks count here, and not in `SameTime`

`Precedence` reads placed **and fixed** occupancy. It has to: a repair run
locks every out-of-scope Session, so a placed-only scan would make a relation
whose predecessor is out of scope silently inert — the "enforces a DIFFERENT
rule than the one configured" failure this ADR names for dangling members,
arrived at from the other direction. `DifferentTime` and `MeetTogether`
already count locks (they read occupancy, and `FixedOccupancy` carries both
relations' row lists).

That leaves the `SameTime` family as the outlier: `constraints::member_week_sets`
walks `placement_ids()` only. There is a defensible reading — including a lock
in a per-week SET-equality check would force the search to match that lock's
exact day and block, a stronger claim than the type makes — and ordering makes
no such claim, so the two genuinely differ. But it has not been *decided*, only
noticed. **Recorded here rather than changed in passing**: altering a shipped
type's semantics belongs in its own change with its own tests.

## The day-counted gap family is a third bound, not two more kinds (issue #55)

`Next Day` and `Two Days After` were two of the "three types in forty" this ADR opened
by using to justify an ordered set. Both are now built, and neither is a type:
**they are values of one new scalar, `Precedence.min_days_between`** — a FLOOR on the
same boundary `max_days_between` already ceilings, computed from the same `days`
expression, in the same calendar days.

That leaves `Precedence` as the only one of the three that was ever a kind, which does
not weaken the ordered-set decision — the order is still read, by the type that reads
it — but it does correct the arithmetic. The ordered set bought one type, not three, and
it was still cheap.

**One scalar rather than two kinds** is [ADR-0024](0024-one-type-per-axis-with-flags.md)
applied without argument: floor and ceiling are two directions of one axis over one
field, which is the case that ADR was written for. Its separate-instantiability objection
carries over intact — two kinds could both be configured over the same member set, and
nothing could stop "next day" and "two days after" being asked for at once. And a kind
would have had to *choose* between "exactly N days" and "at least N days" invisibly,
where two scalars make the tenant say which: `min == max` is exact, a floor alone is
at-least.

### The wall-clock floor is not a substitute, and that is provable

`min_gap_minutes: 1440` looks like "at least a day" and is not. It is wall-clock, so it
constrains time-of-day as a side effect: a lecture ending Monday 12:30 and a lab starting
Tuesday 08:00 are 1170 minutes apart and would breach it, while a same-day pair 150
minutes apart would pass anything below that. A separating threshold exists only inside
`(last_start - first_end, 1440 + first_start - last_end]`, a window derived from
`TimeGrid.day_start_minute`, the block lengths, every break and the block count — so a
value that separates today stops separating when the grid gains a block. **On a teaching
day spanning 12 hours or more the window is empty and no value works at all**, which is
an impossibility rather than a tuning problem, and is pinned by a test.

### What is deliberately still NOT expressible

Two readings of "next day" are refused rather than approximated, and both are refused on
this ADR's own rules:

- **"The next TEACHING day."** In calendar days, `min_days_between == max_days_between
  == 1` means a Friday predecessor demands a Saturday successor — and because
  `Precedence` is term-wide and priced at `hard_penalty`, one Friday Session breaches the
  relation for the whole run, silently. The "Two units, deliberately" section above
  already drew this line: `Daybreak` treats consecutive `day_index` values as adjacent
  nights *because it is about adjacency*, and `Precedence` measures a multi-day distance.
  "Next day" is adjacency; "two days after" is distance. Only the distance half is a
  parameter of this type. A teaching-day unit would be a flag on this axis, and must
  arrive with the request that needs it — picking a day unit wrong is invisible.
- **A per-occurrence pairing.** UniTime's `Next Day` pairs occurrence *i* of one class
  with occurrence *i* of the other. `Precedence` declares its pairing as TERM-WIDE, ALL
  PAIRS, so twelve lectures and twelve labs have ONE boundary, and
  `min_days_between: 1, max_days_between: 1` says "the last lecture is exactly one day
  before the first lab" — a block-teaching statement, not "each lab follows its own
  lecture". This ADR requires each type to declare its own occurrence pairing, so a
  per-occurrence relation is a NEW kind, not a parameter. It stays unbuilt: it is only
  meaningful when the two frequencies match, and Calendry places Sessions per-week
  independently with no meeting-pattern object — the same fact that makes the `SameTime`
  family per-week best-effort.

Also unexpressible, and also unrequested: a SYMMETRIC gap ("N hours apart, either
order"). `min_gap_minutes` imposes the gap *and* the ordering because it lives on
`Precedence`. UniTime's hour-gap types are symmetric. If that is ever wanted it is a flag
on this axis, not a kind — and, like the day unit, it must arrive with its request.

### Reporting

The floor reports under `PrecedenceRelation` like every other breach of this type, as a
fourth `Breach` variant, and it **suppresses the minute-gap check** when it fires — the
same exclusivity `OutOfOrder` already has, for the same reason: a boundary on the wrong
day has no meaningful minute gap to be short, and charging one mistake twice at
`hard_penalty` would misprice it. It does not suppress the ceiling: under contradictory
input both bounds are genuinely breached and the timetabler should see both. Locked and
past Sessions count, because the floor rides `precedence_extents`' existing
placed-and-fixed walk — the divergence from the `SameTime` family recorded above is
unchanged, not widened.
