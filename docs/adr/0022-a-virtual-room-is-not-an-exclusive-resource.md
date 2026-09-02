# A virtual Room is not an exclusive resource

`Occupancy.room` is a `BitMatrix` over `(rooms × slots)`: binary, with no capacity
dimension. Neither `is_free` nor the `RoomDoubleBooking` branch of `check_pair`
consulted `is_virtual`, so once any Session occupied the virtual room at slot S,
**no other Session could be placed there during construction or LNS** — one
online Session per slot, institution-wide. That constrained the *search*, not
merely the report: it changed the placements produced.

Found by a targeted audit from the Nuxt side rather than by a failing test,
because nothing here exercised concurrent online delivery. It had been silently
capping online teaching since slice 1.

An oversight rather than a stance. `is_virtual` was already consulted by
`MinimizeOnline`, by the `allow_online` eligibility gate and by
`SearchState::is_online`; the proto states the intent outright — online delivery
is a Room "so room-assignment logic stays uniform" (see
[ADR-0001](0001-hybrid-heuristic-plus-local-search.md)'s domain framing and
`CONTEXT.md`'s *Virtual Room*); and the generator's own comment calls virtual
rooms unbounded-capacity while the occupancy layer capped each at one. Uniform
room handling was the design — the occupancy layer just never got the exemption
everything else already had.

## One predicate, two layers, no room to drift

`Room::is_exclusive()` is the single definition of the policy.
`Occupancy::exclusive_room()` is the only expression the search consults, and
`mark`, `unmark` and `is_free` all go through it, so they cannot claim a bit the
others do not test. `check_pair` calls `is_exclusive()` directly. Had the two
disagreed, the solver would refuse placements it then declined to report, or free
a bit it never set.

Keyed on the **flag**, never on a well-known "online" room: nothing restricts a
tenant to one virtual room, and the presets ship two to ten of them.

## Consequences

`capacity` still gates *eligibility* in the conversion layer and was deliberately
left alone. A virtual room with a genuine concurrency limit — a single meeting
licence — cannot be expressed today at all, because `capacity` means seats. It
would need an explicit `concurrent_capacity`, not an overload of this flag.

Audited as genuinely isolated to `RoomDoubleBooking`: the lecturer, attendee and
group matrices are marked and queried without reference to `who.room` at all, and
`check_pair`'s other three branches key on persons and groups. That is right on
the merits too — a person cannot attend two things at once whether or not one of
them is online.

One fixture depended on the bug and was rebuilt rather than patched.
`group_day_with_both_room_types` pinned a Session into the virtual room to make
it unavailable at one block, which only worked while virtual rooms were
capacity-1. It now produces its mixed day from **eligibility** — one Offering
permitted online, one not — so it cannot regress the same way.

The measured consequence is its own decision:
[ADR-0025](0025-maxonlineshare-is-not-enforced-by-the-search.md). The cap this
bug was enforcing by accident is now enforced by nothing.

## A second consequence of the same fact: `MinimizeCapacityWaste` (issue #63)

`Problem::capacity_waste_cost` charged every enabled instance by how far a
placement's summed Room capacity exceeded `min_capacity` — with nothing
exempting a virtual Room, so an online placement was priced as though it were
a lecture hall standing mostly empty. Same argument as this ADR's own, applied
to waste instead of to occupancy: a virtual Room has no seats and no scarcity,
so it has nothing to waste either.

Fixed by `Problem::exclusive_capacity`, a single helper (replacing five
duplicated `all_rooms().map(capacity).sum()` call sites) that sums only
non-virtual Rooms. A multi-Room combination mixing physical and virtual now
reads its waste ratio against the physical seats alone; an all-virtual
combination reads `0`, which `capacity_waste_cost` already treats the same way
it treats `min_capacity == 0` — nothing to charge.

## A third: exclusivity BETWEEN Rooms — footprint tags (Calendry #122)

Movable walls. Rooms 1.0, 1.1 and 1.2 sit behind folding partitions; open every
wall and it is the Audimax, close them and it is three independent rooms.
Booking 1.0 must make 1.1, 1.2 **and** the Audimax unbookable for that slot,
and booking the Audimax must make all three unbookable. One physical footprint,
four Room identities.

Nothing in the model could say it, and this ADR is why: exclusivity here is
read from `Room::is_exclusive()`, a property of **one Room against itself
across time**. There was no way to state that booking Room A also occupies
Room B. `MeetTogether` is the opposite pairing (two Offerings sharing one
Room), and multi-room requirement is the reverse again (one Session spanning
several Rooms) — neither is this.

`Room.footprints` is an open-vocabulary tag, like `feature_tags`: two Rooms
overlap when they share one. Chosen over a directed "A also books B" reference
for the reason this ADR keeps insisting on a single predicate — a tag is
**symmetric by construction**, so the two directions cannot be built with one
of them missing. It also answers the ticket's open question for free: a Room
may carry several tags, which is how a wall shared between two combination
options is expressed, and a tag only one Room carries is naturally inert
rather than an error, so a half-entered configuration does not fail a run.

**The footprint is expanded on the QUERY side, never on `mark`.** This is the
load-bearing decision and the one that is easy to get wrong: marking the
siblings' bits too is the obvious implementation, is one line shorter, and
passes every test except one. It makes overlap **transitive**, and overlap is
not transitive. With `A | mid | B` behind two separate folding walls, `mid`
overlaps both while `A` and `B` overlap nothing of each other's; marking would
set `mid`'s bit when `A` is booked, and `B` would then read that bit and refuse
a slot it is entitled to. So `Occupancy::mark` still writes exactly one bit per
assigned Room — a set bit always means "this Room itself is in use" — and
`is_free` asks "is anything I overlap in use", walking `Problem::
footprint_siblings`. `room_footprint_siblings` is resolved once in
`Problem::build` for the reason `room_location` is interned there: the hot loop
gets an indexed read of a slice that is EMPTY for every Room without a folding
wall, which is nearly all of them.

Non-exclusive Rooms are dropped from both sides of the closure, which is this
ADR's own rule reaching one layer further: a virtual Room has no physical
footprint and its occupancy row is never consulted, so a tag on one could only
ever be inert. **Inert is refused, not tolerated** — `ConvertError::
FootprintOnVirtualRoom`. That is the sharpest available version of this ADR's
opening complaint: a caller who believes they declared a hard exclusivity would
get zero violations reported forever while the space was double-booked every
time. Core softens it to "dropped" rather than an error, because core takes
fixtures as well as wire input, and the wire is where the fault can be named.

Reported under `RoomDoubleBooking` rather than a constraint type of its own,
with a message naming both Rooms. The rule is unchanged; only the definition of
"the same room" widened. `check_pair` gets the branch independently of
`Occupancy` for ADR-0014's reason — the search can never produce such a pair,
but a caller's snapshot can, two locked Sessions either side of a folding wall,
and the authoritative checker is what tells the timetabler about it.
