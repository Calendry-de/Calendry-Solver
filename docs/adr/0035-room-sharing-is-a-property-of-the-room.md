# Room sharing is a property of the Room, not a relation between Offerings

`CanShareRoom` is the last unbuilt `OfferingRelation` type from
[ADR-0028](0028-a-relation-is-an-ordered-set-of-offerings.md)'s original list, and
`MeetTogether`'s own schema comment has carried the reason it was skipped since it
landed: *"nothing has requested it independently of the full package, and it would need
its own answer to what 'sharing' means without SameTime/SameDays binding the pair
together."* Issue #55 asks for that answer. **It is that the question has three answers,
none of which is a relation.**

## The three readings

`MeetTogether` binds Room AND time AND sums capacity. Remove the time binding and what
is left is ambiguous, in a way the name does not disclose:

- **Permission** — these Offerings MAY co-occupy one Room where they overlap in time.
  Their pairwise room exclusivity is waived. This is what UniTime's `Can Share Room`
  means, and its `Meet Together` is the package `Can Share Room` + `Same Room` +
  `Same Time` + `Same Days`, which is why this repo's `MeetTogether` is the package too.
- **Preference** — they SHOULD land in the same Room when they can. This is UniTime's
  separate `Same Room` type, a DIFFERENT unbuilt kind. The name collision is itself
  evidence that "CanShareRoom" is too ambiguous to build from the name.
- **Capacity relief** — like `MeetTogether`, but only when they happen to coincide.

## Permission is an exemption with no rule behind it

Every kind on the mechanism either FILTERS (`DifferentTime`, `MeetTogether`) or is HARD
and PRICED (`SameTime`, `SameDays`, `SameStart`, `Precedence`). A permission does
neither: it only widens the feasible set, so it can never be violated, and it has no
evaluator, no `ConstraintType`, no objective term and nothing to report. This ADR's
parent requires each type to declare what it relates and how; a permission has nothing to
declare.

It is also a hole in a HARD structural type, and the hole costs more than the rule.
[ADR-0014](0014-structural-stays-independent-of-occupancy.md) requires the authoritative
structural check to agree with `Occupancy` exactly, so `check_pair` would need a matching
exemption alongside its existing `meet_together_pair` one. `MeetTogether` paid that price
to buy a rule. This would pay it to buy a relaxation.

## Sameness is an equivalence; permission is only symmetric

This is the load-bearing finding, and it is why
[ADR-0022](0022-a-virtual-room-is-not-an-exclusive-resource.md)'s third addendum is the
right precedent rather than a distant one.

`MeetTogether`'s exemption works because `Occupancy` never learns WHO holds an occupied
cell. Its anchor is keyed by `(relation, week)` and pins an exact `(start, end, room)`;
a candidate not matching it exactly is refused up front, so any set bit inside that span
provably belongs to a fellow member — which is what lets the per-slot loop trust a single
`joining` flag instead of re-deriving whose bit it is looking at.

That machinery is transitive, and for `MeetTogether` it is RIGHT to be. With relations
`{A, B}` and `{B, C}`, `B`'s mark anchors both, so `C` may join a cell held by `A`
despite sharing no relation with it — and it should, because **"is the same physical
meeting" is an equivalence relation**, and `meet_together_cells` prices all three against
the Room's seats.

**"May share a room with" is merely symmetric.** If A may share with B and B with C, A
may not thereby share with C: permissions do not compose. So a `CanShareRoom` built on
the existing anchor would make a non-transitive relation transitive — the exact error
ADR-0022's third addendum exists to prevent, arrived at from a different direction.
Building it correctly means knowing, per `(room, slot)`, WHICH Offerings hold the cell,
and testing the candidate pairwise against every one of them. That is a new per-cell
holder structure, a `mark` that records the holder's identity, and a query-side pairwise
test on `is_free`'s reject path. Recording the actual holder is not ADR-0022's sin —
that addendum forbids writing DERIVED bits at mark time, and the holder is a fact, not a
derivation — but it IS new mechanism, for a feature nobody has asked for.

The invariant, for whoever eventually does ask: every pair of holders of a cell shares a
relation, maintained inductively by testing each entrant against all current holders. And
the exemption applies to the IDENTICAL Room only, never across a footprint — a fellow
member holding 1.0 must not entitle you to the Audimax, which physically contains it.
Today's code has that property by construction and a test now pins it.

## Capacity relief is the Room axis, and the missing field is already named

If the real want is "this hall holds two seminars at once when they fit", that is not a
statement about a pair of Offerings — whether a Room holds two Sessions depends on their
SIZES, not their IDENTITIES. And it is genuinely unrepresentable: `Room::is_exclusive()`
is `!is_virtual`, so there is no non-exclusive PHYSICAL Room, and `is_virtual` cannot be
borrowed because it also means "online" and is read by the day-mix term,
`MinimizeOnline`, `MaxOnlineShare` and `MaxConcurrentOnlineSessions`.

ADR-0022 recorded this gap from the other side in the same doc comment: *"a virtual room
with a genuine concurrency limit (a single meeting licence, say) cannot be expressed today
at all … Expressing a real cap needs its own field, not an overload of this flag."* The
virtual half was later closed by `MaxConcurrentOnlineSessions`, which caps per-slot
concurrency and CAN be a filter because a per-slot count has no moving denominator. **The
physical equivalent is the same shape, per Room.** That is where capacity relief belongs,
it needs its own request and its own evidence, and it must not arrive disguised as a
relation kind.

## Consequences

No wire change, no new `RelationKind`, no `ConvertError`. The `MeetTogether` schema
comment gains a pointer here instead of carrying the reasoning inline.

`SameRoom` — the preference reading — remains genuinely unbuilt and genuinely additive:
one evaluator on the hard-priced side of the mechanism, alongside the `SameTime` family,
comparing per-week Room sets rather than `(day, block)` sets. It is cheap if it is ever
requested. It is not this.

The refusal is pinned by tests rather than left here, `crates/core/tests/
can_share_room.rs`, on the pattern `crates/core/tests/mid_week_absence.rs` set for
Calendry #118: the reasoning is executable, because the cost of getting it wrong is a
schema change across three repos plus a hole in a hard rule, and the next reader to reach
for the `MeetTogether` anchor should fail a test rather than ship a transitive permission.
