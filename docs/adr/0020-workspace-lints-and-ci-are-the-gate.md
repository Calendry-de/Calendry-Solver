# Lints are configured once in the workspace manifest, and CI is the gate

There was no CI in this repository. Clippy ran only when a human typed the
command, and the command on record omitted `--all-features`, `--locked` and
`-D warnings` — so a warning was advice, not a failure. There was no
`[workspace.lints]` table either.

Lints live in the root manifest and are inherited via `[lints] workspace = true`,
so a new crate inherits the policy instead of quietly opting out of it. CI runs
rustfmt, clippy with `-D warnings`, the tests, rustdoc with `-D warnings`, and a
release benchmark smoke run that exercises the preset calibration assertions.

`rustfmt.toml` is tuned to the style the repository was already written in rather
than reformatting 10,000 lines to match the defaults: the code makes heavy use of
single-line struct literals for dense index types, and breaking those across four
lines each hurts more than it helps.

## Consequences

`crates/proto/src/lib.rs` relaxes several lints for the generated module alone.
Everything in it is machine-written from the `.proto` sources, so a lint there
could only be satisfied by editing a generator template in another repository.

## `missing_docs` is deliberately not enabled

The Apollo handbook asks for `#![deny(missing_docs)]` on libraries. Measured
before deciding: it produces **575 warnings**, of which **304 are struct fields**
(`Room.id`, `Room.name`, `Person.role_tags`, …). Satisfying those means writing
304 comments of the form `/// The id.`, which adds no information and buries the
type- and module-level documentation that does.

Not enabled, therefore. What is enabled instead is the **rustdoc job with
`-D warnings`**, which catches the one documentation defect that rots silently: a
broken intra-doc link, because nothing else ever resolves those paths. It found
three on its first run — two pre-existing, one introduced by making `Occupancy`
private.

Revisit if the crate ever ships as a public API, where a field with no
explanation is a genuine cost to a reader who cannot open the source.

## `protoc` is a build dependency nothing declares

The first CI run failed in four jobs at once with `Could not find protoc`, after
the workspace had been green locally all session. That is not a flake and it is
worth recording, because the shape recurs: **`protoc` is a build dependency of
`crates/proto` that appears in no `Cargo.toml`**, so `cargo build` succeeds on any
machine that happens to have it and there is no local signal that it is missing.

`.github/actions/rust-build-env` is the single definition of what a compiling job
needs — the pinned submodule, protoc, a toolchain, the cache. Four jobs compile
and a fifth would have forgotten one of them. `fmt` deliberately stays outside it:
it parses rather than compiles, so it needs none of that and remains a fast first
signal.

Sourced from **apt, matching the `Dockerfile`**, rather than from a third-party
setup action — so the image build and CI cannot drift onto different protoc
versions. The version floor is real: the schema uses proto3 `optional`, which
needs protoc >= 3.15; Debian and Ubuntu ship 3.21. The action prints
`protoc --version` so a silent downgrade is visible in the log.

The composite action initialises the submodule with `--init --recursive` and
**never `--remote`** ([ADR-0003](0003-proto-schema-as-a-pinned-submodule.md)), and
checks for the checkout separately from installing protoc — the two produce
similar-looking `build.rs` failures, and naming which one it was costs one line.

Considered and rejected: vendoring protoc into the build (via `protobuf-src` or a
`protoc-bin-vendored` dependency) would make the build hermetic and remove this
class of failure entirely. It also pins a protoc version in `Cargo.toml`, builds
it from source on some paths, and diverges from how the Dockerfile sources it.
The failure mode here is loud and immediate rather than silent, so consistency
with the image won.
