# The protobuf schema lives in a separate repo, consumed as a pinned submodule

The contract is shared with the Nuxt app, so it lives in a language-neutral
repository (`github.com/Calendry-de/calendry-proto`) and is consumed here as a
git submodule at `vendor/calendry-proto`, pinned to an exact revision. `.proto`
files are never copied into this repo.

`crates/proto/build.rs` resolves the checkout in exactly two ways: the
`CALENDRY_PROTO_DIR` environment variable, for CI and reproducible builds, or the
submodule. **There is deliberately no sibling-checkout fallback.** An earlier
revision fell back to `../calendry-proto/proto`; it was removed because a sibling
checkout is unpinned and unversioned, so the fallback let the build succeed
against whatever happened to sit in a neighbouring directory whenever the
submodule was missing. That is precisely the silent schema-drift failure the pin
exists to prevent. Do not reintroduce it.

Pin to **tagged** commits, not loose SHAs. The tag is what ties the two consumers
together: this repo pinned at the commit tagged `v0.2.0`, and the app installing
`@mindcollaps/calendry-proto@0.2.0` built from that same tag, means "which schema
is each side on" has one answer.

## Consequences

Updating the schema is a deliberate act producing a one-line reviewable diff
(`-Subproject commit …` / `+Subproject commit …`). `branch = main` in
`.gitmodules` affects **only** `git submodule update --remote`; that flag must
never appear in a build script or in CI.
