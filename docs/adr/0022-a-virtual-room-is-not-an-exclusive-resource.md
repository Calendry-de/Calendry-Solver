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
