# Four crates, so that `core` cannot see prost types

Generated protobuf types are `String`-id'd, `Option`-wrapped and heap-heavy. A
local search evaluating millions of candidate moves cannot touch that
representation; it addresses everything by dense `u32` index into flat arrays.

The split is `core` (domain model, indices, evaluators, search — no prost, no
tonic, no tokio, no I/O, no clock), `proto` (codegen only), `service` (the tonic
server, the run registry, and the proto↔core conversion), and `gen` (the
benchmark generator and harness).

Keeping them as separate crates makes erosion of the boundary a **compile
error** rather than a convention someone relaxes with one "just read the id off
the message".

## Consequences

Conversion is a real module with real work in it, and lives in `service` rather
than in its own crate — see ADR-0018 for the condition under which that changes.
