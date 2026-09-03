# A Room pin is checked against the candidate, not precomputed into the Offering

"The workshop lead always teaches in the workshop" (Calendry #124). A Person is
bound to a Room or a small set, and every Session they lead is placed there.

The ticket files this as a change to the solver's model, for a structural reason
that is true as far as it goes: Room eligibility is precomputed per Offering in
`build_offerings`, before lecturer assignment, so a person-scoped Room set makes
the eligible-Room set lecturer-dependent — and where the search *chooses* the
lecturer, it cannot be computed once per Offering at build time.

It is contained anyway, because the prefilter is not what enforces anything.

## The decision

**`Person.allowed_room_ids`, checked in `SearchState::statically_blocked`
against the candidate's CHOSEN lecturers and ALL of its Rooms.** HARD, a filter,
enabled by a `LecturerRoomPin` constraint type, and stored as its complement.

## The prefilter is an optimization; the gate is the mechanism

Construction nests the lecturer loop inside the room loop, and repair addresses
the full `starts × rooms × lecturers` cross product by index. Both converge on
one feasibility gate, `SearchState::is_free`, and both have already attached the
candidate's Rooms **and** its chosen lecturers to the `Occupant` before asking —
`with_room` / `with_additional_rooms` / `with_pool_lecturers`, read back through
`Occupant::all_rooms` and `Occupant::all_lecturers`.

So the pin is one predicate over data already in hand, in `statically_blocked`:
"the occupancy-independent half […] everything that rejects a cell no matter what
else is placed", which is exactly what a Room pin is. `individually_eligible`
keeps its current definition — a sound superset, and for every Offering without a
lecturer pool the identical set it produces today.

Putting it there rather than in `Occupancy::is_free` also gives ADR-0031's
targeted ruin operator the right answer for free: a cell a pin rejects has no
blockers worth evicting.

## `LecturerVeto` is the wrong sibling to copy, and copying it is invisible

CLAUDE.md files `LecturerVeto` as unary in `(slot, lecturers)`, which makes it
look like the model: a pin is unary in `(rooms, lecturers)` the same way. The
*rule* is the same shape. The *implementation* is the trap.

`Offering::veto_slots` is a per-Offering slot mask built once in `Problem::build`
from `veto_mask(&o.lecturers)`. A genuine pool Offering's `Offering::lecturers`
is **empty**, so that mask is unconditionally empty and can never block anything
— which is why **`LecturerVeto` combined with a pool is refused at conversion**,
the one instance of ADR-0026's precomputation trap that was never fixed.

A `pinned_rooms: BitSet` on `Offering`, built the same way, is shorter than the
predicate above and passes every fixed-assignment test. It is **silently
permissive for exactly the pool case this feature exists for.** The app already
honours the fixed case by intersecting the pin into `Offering.allowed_room_ids`
at assembly, and *reports* the pool case it cannot honour; closing that report to
zero is the entire point of the solver-side work.

So the guard is a mirrored pair over one fixture, the mechanism ADR-0027 uses:
`a_pin_binds_a_fixed_assignment` passes under both implementations and exists
only so its mirror is not vacuously green, and `a_pin_binds_a_pool_offering`
passes under only the correct one.

The right sibling is `PersonPreferenceFit`'s pool path: a per-person table read
live, with `Problem::preference_cost_for_placement` as the one place that picks
between the precomputed and the live path. A pin is that shape on the
`(person × room)` axis, and cheaper — a bit test, not a mean.

## HARD and a FILTER, not priced

Both of this repo's priced-hard precedents are priced because they *cannot* be
filtered, and each states the property it lacks. `MaxOnlineShare` (ADR-0025) is a
cardinality ratio, invisible in any pair, whose denominator moves under
`PER_WEEK`. The `SameTime` family and `Precedence` (ADR-0028) compare per-week
sets over an Offering's whole placed set and are not decidable mid-search from
partial state. `OnlineOnsiteSameDay` (ADR-0023) became priced because a mixed day
is genuinely sometimes the right answer.

A Room pin has none of those properties. It is candidate-local and monotone: no
eviction anywhere can make a forbidden Room permitted. Pricing it at
`hard_penalty` would add an `Objective::soft` term, delta accumulation, a
breakdown row and drift-assertion surface — and the search could still cross it,
so a run would report success with the workshop lead in a lecture hall. ADR-0025
accepts that for `MaxOnlineShare` because it has no choice. Here there is one.

The counter-argument is CLAUDE.md's own: the solver tolerates infeasible input,
because the app's warn-and-allow UX produces it, and a hard pin lets one edit to
a Room make an Offering unplaceable. The answer is that "tolerates" already means
*reports*, not *ignores* — an unplaced placement is a documented legitimate
outcome, it surfaces as `ExactFrequency`, and since ADR-0031 such a run can never
claim `converged`. The same blast radius already exists for
`Offering.allowed_room_ids` and for a term-long `LecturerVeto` blackout, and the
repo has never answered it by softening a rule.

**And the soft version is already built.** `Preference.preferred_room_features`
is converted, tabled on both the per-placement and per-person paths, and priced
live against the chosen Room's features — although the proto comment and
`docs/SCHEMA.md` both still said it was unbuilt, and both are corrected in this
change. So a soft `PersonRoomAffinity` would be a second type on an axis that
already has one, which ADR-0024 forbids; a tenant wanting the soft behaviour can
tag the workshop with a feature and put that feature in the lead's preferences
today. Hard is not the better of two options for #124 — it is the only thing
#124 adds.

## A field on `Person`, and a type to switch it on

Both, because every per-entity fact that narrows placement in this schema is
stored on the entity and enabled by a tenant-policy type: `Person.blackouts` +
`LecturerVeto`, `Group.blackouts` + `GroupVeto`, `Room.is_specialized` +
`MinimizeSpecializedRoomUse`, `Offering.required_session_count` +
`ExactFrequency`.

Calendry #123 argues room pinning is Offering *data*, not tenant policy, exactly
as `required_room_count` is — and for an Offering that is right, since an
Offering's Room set is definitionally part of what the Offering is. It does not
carry to a Person. A pin reaches every Offering that Person leads, across kinds,
and the decisive axis is `applies_to_kinds`: a tenant wants the workshop lead
pinned for `lecture` and `lab` and not for `staff_meeting`, and a bare `Person`
field has nowhere to say so. A hard per-entity rule that can make a run
infeasible also needs an off switch and an id to report against.

The near-miss is `Room.footprint_tags`, which is per-entity data with no type of
its own, reporting under `RoomDoubleBooking` (ADR-0022). That works because it
*widens an existing rule's* definition of "the same room" — the rule already had
an id, an enablement and a kind scope. Nothing here says "a lecturer's Sessions
happen in their Rooms" for a pin to widen.

Named for **lecturers**, not persons, because the counted set is lecturers only —
ADR-0026's scope decision, followed without exception: a pinned Person who only
*attends* is inert, and `Occupant::all_lecturers` is the only set consulted.

## The pin arrives as a whitelist and is stored as its complement

`Person.allowed_room_ids` empty means **every Room**, mirroring
`Offering.allowed_room_ids` — the convention #123 calls the sharpest trap in the
product. Every mask in `Problem`/`Offering` has the opposite polarity: empty
blocks nothing (`veto_slots`, `group_veto_slots`, `protected_block_slots`,
`charged_specialized_rooms`, `footprint_siblings`).

`Problem::build` therefore inverts once, into `person_room_veto` — the Rooms each
Person may **not** teach in, and an empty `Vec` when no Person states a pin at
all. After that, "empty blocks nothing" is true of this mask exactly as it is of
the other five, and no read site has to remember which polarity it holds. The
trap dies at one line rather than living at every use. A stated-nothing pin
yields an empty row; an unsatisfiable pin yields a full one; the two are never
confusable, which is what stops a solver-side empty set from ever being read as a
widening.

One implementation detail that is not free to get wrong: an unpinned Person's row
is full-width-and-empty, **not** zero-capacity. `BitSet::contains` debug-asserts
its index against the capacity, so a narrowed row panics the moment anything asks
about it. The saving that matters is the empty outer `Vec`, which covers every
tenant that pins nobody — that is where the "costs nothing when unused" property
actually lives.

## Every Room, not at least one

A Session needing 2–4 Rooms must satisfy the pin in **all** of them. "At least
one" would let a pin be escaped by asking for more Rooms — a hard rule that gets
weaker as the request gets bigger, which nothing else in this catalogue does —
and a multi-Room Session occupies all of its Rooms simultaneously, so the lead
teaching in the workshop plus an ordinary seminar room is teaching partly outside
the pin. "Every" is also what keeps the filter monotone, so pruning stays sound.

The deliberate divergence: the *soft* room-feature term reads the primary Room
only, and ADR-0024 charges `MinimizeSpecializedRoomUse` once per placement to
keep `hard_penalty`'s per-placement ceiling exact. Both are ceiling arguments
about a price. A filter has no ceiling to preserve.

## A pin never expands through a footprint

ADR-0022's rule is *expand the question, never the answer*. A footprint expands a
**blocking** query, because overlap in space is overlap in occupancy. A pin is a
**permission**, and permissions never expand: the Audimax is not the workshop,
whatever wall they share. Booking the pinned workshop still blocks its siblings
for everyone else, through the existing query-side expansion, unchanged.

This is the detail a reader fresh from ADR-0022 is most likely to "fix", so it
has a test of its own.

## Refuse the inert, report the loud

The classification rule behind every degenerate case.

* An **unknown Room id** in a pin is **refused** (`ConvertError::UnknownRoom`,
  context naming the Person). Dropping it would *widen* a whitelist — drop the
  last id and the pin silently becomes "any Room" — and `build_persons` already
  refuses an unknown `group_ids` for the mirror reason.
* A pin naming a **VIRTUAL Room** is **honoured**, not refused. ADR-0022 refuses
  a footprint tag on a virtual Room because its occupancy row is never read, so
  the tag could only be inert. A pin is not inert on a virtual Room: its
  *identity* is read by `Offering.allow_online`, by `MinimizeOnline` and by the
  placement itself. "This person only ever teaches online" is a real pin. The
  reasoning is one word from being misapplied, which is why it is written down.
* An **empty intersection** — a pin disjoint from an Offering's eligible Rooms,
  or two pinned lecturers with disjoint pins on one Session — leaves the Session
  **unplaced and reported**, never refused and never relaxed. Not refused
  because with a lecturer pool it is not decidable at conversion at all: which
  candidates co-occur is a search-time choice, so a refusal would reject a
  request the search would have solved by choosing differently. Not refused,
  too, because the identical shape is already tolerated one field over — an
  `Offering.allowed_room_ids` naming only Rooms that fail the capacity filter
  yields an empty eligible set with no complaint.

  An empty intersection is the *opposite* of `FootprintOnVirtualRoom`'s fault:
  it is maximally loud. Nothing places, `ExactFrequency` fires, and ADR-0031
  forbids the run from claiming convergence over it. Loud is tolerable; silent
  is not.
* A **pinned Person who only attends** is inert, per ADR-0026's scope, and is
  not a fault: a run scoped to one term legitimately contains people who lead
  nothing in it, and a workshop lead who also sits in staff meetings must not
  have those meetings pinned.

## Consequences

* **The pool case is supported rather than refused**, which is the whole delta
  over `LecturerVeto` and the reason no analogue of its pool refusal is added.
  That absence has its own test, in negative space, next to the refusal it
  deliberately does not copy.
* **No objective term.** `Objective::soft` is untouched, the aggregate-drift
  assertion gains nothing to drift, `ruin_worst` gains no blind spot, and the
  breakdown gains no row. Filtering is cheaper than pricing here in machinery as
  well as in cycles.
* **The authoritative check reports placed placements only**, matching
  `lecturer_veto` and `group_veto` rather than `structural`'s `collect_views`. A
  locked Session's Room was chosen by the caller, who has the app's own checker.
  Both stances exist in this repo — `Precedence` counts locked Sessions and the
  `SameTime` family does not — so this is a choice for consistency with the two
  nearest siblings, reversible by swapping one iterator in one function.
* **`individually_eligible` is left alone.** Narrowing it to the union over an
  Offering's candidate lecturers' pins is sound and exact for every non-pool
  Offering, and is deferred anyway: it is a third expression of one rule that
  can drift from the other two (ADR-0022), and it would shift construction's
  most-constrained-first ordering, which reads `eligible_rooms.len()` and
  decides every preset's output. If a measurement asks for it, it belongs in
  `Problem::build` so the fixtures and the generator get it too — not in the
  conversion layer, where only the wire path would (ADR-0021).
* **Generated instances declare no pins**, so no preset moves and
  `docs/PERFORMANCE.md` is unchanged — ADR-0027's stance for Group blackouts,
  for its reason. No sixth axis is added to the benchmark's construction
  attribution either: with no pin in any generated instance it could only ever
  print `0.00%`, which is the failure that probe's own comment records for an
  axis it already removed.
* **Two documents were wrong and are corrected**: the proto comment on
  `Preference.preferred_room_features` and `docs/SCHEMA.md` both said the
  room-feature preference was unbuilt. It has been built since lecturer-pool
  selection landed, on both the per-placement and the per-person path. The
  correction matters beyond tidiness: while those two sentences stood, "add a
  soft `PersonRoomAffinity`" looked like an open option rather than a duplicate
  type.
