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
