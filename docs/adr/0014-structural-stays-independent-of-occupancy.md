# The authoritative structural check stays independent of the occupancy index

`constraints::structural` reads the same attendee lists the search reads, but it
never consults `Occupancy`. It is the authoritative pairwise check; `Occupancy`
is an index the heuristic uses to *avoid* creating violations, and is documented
as a conservative approximation.

This is knowingly redundant work. Measured on both university presets, **every**
structural violation involves two immovable Sessions and none involves a placed
one (80 of 80, and 9 of 9) — which is provable, not incidental, because occupancy
is seeded from immovable input, repair only places where `is_free` accepts, and
the mark/query semantics match `check_pair` exactly on all four axes. So ~99.75%
of the pairwise scan cannot report anything.

**It was deliberately not exploited.** Restricting the scan to immovable pairs
would make the authoritative check depend on the correctness of the thing it
exists to verify — and that safety net has already caught two real search
defects. Worth revisiting only as a debug-gated fast path, never as a
replacement.
