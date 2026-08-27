# Conversion errors are typed; the mapping to gRPC lives in one place

Every validation predicate in `convert.rs` used to construct its own
`tonic::Status` in place, at 21 sites. Three costs came with that: the transport
type *was* the conversion module's error type, so validation could not be
exercised without linking `tonic`; the code-selection policy had no single home
and had already drifted; and core's typed errors were flattened to prose, so a
caller wanting to distinguish "your group hierarchy has a cycle" from "your time
grid is malformed" had to match on message text.

`ConvertError` names the **domain** fault. `impl From<ConvertError> for Status` is
the only place a fault becomes a transport response, and `is_unimplemented` is
the one predicate deciding between `INVALID_ARGUMENT` and `UNIMPLEMENTED`.

ADR-0004 deferred promoting conversion to its own crate until "its validation
logic grows". 21 validation sites in a 700-line implementation is that growth.
This is the step that makes the split mechanical — a `calendry-solver-convert`
crate would depend on `core` and `proto` but not on `tonic` — and deliberately
stops short of taking it.

## Consequences

Unknown-id resolution now goes through `Resolver`, which names its policy per
call: `require` or `optional`. The module's doc comment claimed strictness about
"a room id that does not exist" while four sites silently dropped unknown ids via
`filter_map` — and one of them turned a bad `room_id` into **roomless occupancy**,
structurally invisible to room double-booking. An *empty* room id remains
permitted: an online-only or not-yet-roomed Session is a real state.
