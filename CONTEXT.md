# Calendry Solver

The domain language of Calendry's optimization service. Calendry is a
multi-tenant timetabling platform; this repository does the scheduling maths and
the Nuxt application owns everything else.

The vocabulary below is deliberately split in two. The **domain** section is a
fixed core taxonomy: entity types and their relationships, which change only by
migration. Tenants extend the *values* filling those entities freely — role
names, equipment tags, session kinds, constraint parameters — and never touch
the schema. The **architecture** section is the shared language for designing
modules, and is not domain-specific.

Decisions live in [`docs/adr/`](./docs/adr/), not here. This file is a glossary
and nothing else: no implementation details, no measurements, no roadmap.

---

## Organization

**Federation**:
An optional parent grouping of Tenants that share resources — a university
consortium sharing a lecture hall, or a cross-enrolled elective. Owns resources
member Tenants may reference.

**Tenant**:
A single institution. Fully data-isolated except for explicitly
Federation-owned resources.

---

## People, roles, grouping

**Person**:
The only person entity. There is no separate Student, Lecturer or Staff type.
_Avoid_: user, student, member (as an entity)

**Role**:
Tenant-defined vocabulary attached to a Person. `Lecturer` is the one fixed,
universal role name: the Person leading a Session. Everything else — Student,
Auditor, TA, External Participant — is tenant-defined.

**Group**:
A class, cohort or seminar group. **Nested**: a parent/child hierarchy, typically
Cohort → Class → Seminar Group.
_Avoid_: cohort, class, section (as the entity name — those are *instances* of a
Group at particular depths)

**Membership**:
The Person ↔ Group relation.

**Conflict closure**:
A Group's own id, plus every ancestor **and** every descendant. What "is Group G
free at time T" has to be asked against, because a scheduling conflict on a
parent Group propagates to block its children and vice versa.
_Avoid_: group tree, hierarchy walk

**Subtree**:
A Group's own id plus its descendants only. What **attendance** resolves against:
a cohort Session implicates its classes' members, but a class Session does not
implicate the cohort.

**Attendee**:
Everyone in the room for a Session — direct participants plus the members of its
Groups and their subtrees.

**Participant**:
A Person assigned to a Session individually, independently of any Group
membership. Distinct from Attendee because output must report who was assigned,
not everyone who happens to be present.

**Blackout**:
A window in which a Person is unavailable. Enforced for Sessions they *lead*,
never for Sessions they merely attend. An empty list on an axis means "every
value on that axis".
_Avoid_: unavailability window, veto (the *constraint* is the veto; this is its
data)

**Preference**:
Days and blocks a Person would *rather* have. Counted for Sessions they *lead*,
like a Blackout — but **empty means no preference**, the inverse of a Blackout's
emptiness, and there is no week axis because a Preference is a recurring weekly
shape rather than a dated absence. Soft: a Placement that misses one is priced,
never refused.
_Avoid_: soft blackout, preferred availability (a Preference is not a window of
availability and the two are separately stated)

**Weight multiplier**:
A bounded per-Person override of the tenant's `PersonPreferenceFit` weight —
"this person's preferences count half as much / twice as much as normal".
Dimensionless on purpose, so it survives a change to the tenant weight; bounded
on purpose, so a tenant-editable value cannot reach the derived hard penalty.
Absent is a distinct state from zero.
_Avoid_: priority, seniority (it is a scheduling weight, not a claim about the
person)

---

## Space

**Room**:
A physical or virtual place a Session can occupy, with a capacity, a location and
a rank.

**Rank**:
A Room's desirability, ordered **higher = more premium and scarce**.

**Feature**:
A tenant-defined tag on a Room — projector, PC lab, lab bench — that an Offering
may require.
_Avoid_: equipment, amenity, tag

**Virtual Room**:
How online delivery is modelled. Online is a Room, not a boolean on a Session,
which keeps room assignment uniform. **Not an exclusive resource**: any number of
Sessions may occupy one in the same slot.
_Avoid_: online flag, remote mode

**Exclusive Room**:
A Room only one Session may occupy at a time — every physical Room, and no
virtual one. The single property the room double-booking check consults.

**Footprint**:
The physical space a Room stands in, named by a tenant-defined tag. Several
Rooms may share one: 1.0, 1.1 and 1.2 behind folding partitions plus the
Audimax they combine into are four Rooms over one footprint, so booking any
one of them occupies all four for that slot. Exclusivity BETWEEN Rooms, where
*Exclusive Room* above is a Room against itself. Symmetric, and deliberately
NOT transitive: a Room may sit in two footprints without joining them.
_Avoid_: room group, combined room, room set

---

## Scheduling

**Offering**:
The **demand** definition: this needs to happen N times, needs a Lecturer with
role X, a Group, a Room with feature Y, of kind Z. The solver's input.
_Avoid_: course, module, subject

**Session**:
One **atomic, placed instance** — a specific week, timeslot, Room, Lecturer(s)
and Group(s). What gets displayed, moved, swapped, locked, exported and notified
about. The solver's output, and what manual edits operate on.
_Avoid_: event, booking, meeting

**Kind**:
Tenant-defined vocabulary on an Offering or Session, replacing any fixed
Lecture/Exam/Event split. Every constraint type declares which kinds it applies
to, because a tenant-defined kind — `staff_meeting`, say — may have no Group at
all.
_Avoid_: type, category

**Assignment**:
A Session's relation to a Group, Person, Room or Lecturer.

**Placement**:
A candidate position for one Session: a start slot and a Room. What the search
manipulates.

**Placement variable**:
One Session the current run has to position — an (Offering, occurrence) pair.
Distinct from a Session, which is already placed.

**Immovable**:
Occupancy the run may not move, together with the reason it may not: **Locked**
(explicit user lock, absolute), **Past** (starts before the reference slot,
absolute), **OutOfScope** (nobody asked about it), or **External** (another
Tenant's use of a Federation-shared Room).
_Avoid_: fixed, frozen, pinned

**Scope**:
Which Offerings a run is actively placing. Everything outside it is Immovable.

**Lock policy**:
What happens to occupancy outside Scope.

**Generation**:
An immutable, versioned snapshot produced by a solver run. Manual edits are an
append-only event log applied on top of one.

---

## Time

**TimeGrid**:
A Tenant's configured time structure: block length, blocks per day, active days,
start hour. There is **no global grid**, so every question about time resolves
against the requesting Tenant's own.

**Block**:
The atomic unit of the TimeGrid. A Session occupies one or more consecutive
blocks within a single day.

**Slot**:
One block of one day of one week, flattened to a single index.

**Span**:
The consecutive slots a Session occupies. It never crosses a day boundary.

**Academic calendar**:
Terms, holidays, break weeks and exam periods, as structured data. Anything that
reasons about "exam week" or "holiday" resolves against this.

**Institution-local time**:
The single timezone all solving and grid logic happen in. Per-Person timezone is
a presentation and export concern only, and must not reach "same day" or
"adjacent slot" reasoning.

---

## Constraints

**Constraint type**:
One of a fixed set of predefined kinds of rule, each with one compiled evaluator
reading its own typed parameters. Tenant-supplied logic never executes. Adding
one is a code change; the set is not open to tenants.

**Constraint instance**:
One configured use of a type: an id, the kinds it covers, and its typed
parameters. A type may be configured more than once with different kind scopes.

**Hard constraint**:
A rule defining feasibility. Hard-versus-soft is a property of the *type*, never
a per-tenant setting.

**Soft constraint**:
A rule contributing a weighted penalty to the objective.

**Objective**:
What the search minimizes: unplaced Sessions and aggregate violations on the hard
side, weighted soft penalties on the soft side. Terms belonging to a *set* rather
than to one Placement — a violated share cell, a mixed day — are read off the
running counters instead of accumulated as per-Placement deltas.

**Occupancy**:
The entity-by-slot index recording what is busy when. An index the search
consults to *avoid* creating violations — not the authoritative check.

**Violation**:
A reported breach of a hard constraint, naming its type, the Sessions or
Offerings involved, and a human-readable detail. A **priced** outcome is not a
Violation, however undesirable — it appears in the objective breakdown instead.

**Structural constraint**:
One of the four double-booking types — Room, Lecturer, Group, Person. Pairwise,
keyed by (entity, slot).

**Unary constraint**:
A constraint depending only on one Session's own slot and Room. The six soft
types, and `LecturerVeto`.

**Per-placement constraint**:
A constraint depending only on the candidate Placement, like a Unary one, but
whose cost varies per *Placement* rather than per Kind — so it cannot share the
unary types' `(profile, slot, Room)` table. `PersonPreferenceFit` alone, keyed by
`(placement, day, block)`. Still accumulated as an exact delta, and still
rankable by the ruin operators, which is what separates it from an Aggregate.

**Aggregate constraint**:
A constraint over a *set* of Sessions, not expressible as a slot-keyed bitset:
`OnlineOnsiteSameDay` and `MaxOnlineShare`. Neither can be a filter, so both live
on the objective; they differ only in what they are charged.

**Unmet fraction**:
What `PersonPreferenceFit` charges: the share of a counted lecturer's *stated*
preference axes that a Placement does not satisfy, averaged over the lecturers.
Charged rather than rewarding the met share, so that no soft term is ever
negative. A lecturer who stated nothing is not counted at all, which is a
different fact from one who stated something and got none of it.

**Mixed day**:
A `(Group, day)` cell holding both online and on-site Sessions. Priced at its
configured weight rather than forbidden, so the search will accept one when every
alternative costs more.

**Budget**:
What ends a run. A **move budget** counts evaluated candidate moves and is
reproducible; a **wall-clock budget** is not, because the iteration count depends
on the machine.

**Saturation**:
How hard a generated benchmark instance is, measured on the **binding axis** —
the maximum of room, Group, Lecturer and person-clique load.

**Person-clique load**:
The demand of a set of Offerings that pairwise share an attendee, over the term.
A graph-colouring bound rather than a load figure. Above 1.0 it is a
**certificate of infeasibility**; below 1.0 it proves nothing.

---

## Architecture

The shared language for designing modules here. Not domain-specific — use these
words exactly, and prefer them to the alternatives listed.

**Module**:
Anything with an interface and an implementation. Deliberately scale-agnostic: a
function, a struct, a Rust module, a crate, or a slice spanning several.
_Avoid_: component, service, unit

**Interface**:
Everything a caller must know to use a Module correctly — the type signature, but
also its invariants, ordering constraints, error modes, required configuration
and performance characteristics.
_Avoid_: API, signature (both too narrow: they cover only the type level)

**Implementation**:
What is inside a Module.

**Depth**:
Leverage at the Interface: how much behaviour a caller or test can exercise per
unit of Interface they have to learn. A Module is **deep** when a lot of
behaviour sits behind a small Interface, **shallow** when the Interface is nearly
as complex as the Implementation.

**Seam**:
A place where behaviour can be altered without editing in that place. The
*location* at which a Module's Interface lives.
_Avoid_: boundary (overloaded with DDD's bounded context)

**Adapter**:
A concrete thing satisfying an Interface at a Seam. Describes the role it fills,
not what is inside it. One Adapter means a hypothetical Seam; two means a real
one.

**Leverage**:
What callers gain from Depth — more capability per unit of Interface learned.

**Locality**:
What maintainers gain from Depth — change, bugs, knowledge and verification
concentrating in one place instead of spreading across callers.
