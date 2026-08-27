# One constraint type per axis, with flags, rather than one type per direction

`MinimizeBlockUsage` replaces `MinimizeFirstBlock` and `MinimizeLastBlock`, which
were two directions of one axis over one field, and adds arbitrary block indices
alongside `first`/`last` flags. `MinimizeRoomRank` gains an `invert` flag rather
than a second type for the opposite direction.

Two types would also be **separately instantiable**, so a tenant could enable
both and penalize a room — or a block — from both ends at once. Nothing could
prevent that, because each type carries its own instances.

Both directions of the rank rule are real policies: an institution may want its
best halls kept free for events, or may want them *used* for teaching rather than
standing empty while lessons go into the cheap rooms.

## Consequences

The deprecated messages are retained on the wire, because removing a field is a
breaking change and `buf breaking` rejects it
([ADR-0003](0003-proto-schema-as-a-pinned-submodule.md)). Senders should emit
`MinimizeBlockUsage`; this repo's own test fixtures were migrated off the
deprecated pair, and the compiler's `deprecated` warning is a hard error under
the workspace lint policy
([ADR-0020](0020-workspace-lints-and-ci-are-the-gate.md)), so a new use cannot
land quietly.

A rule that selects nothing at all is **rejected** rather than run: an empty
`MinimizeBlockUsage` with neither flag set can only be a configuration mistake,
since it carries a weight, costs scoring time, and can never fire.

`MinimizeRoomRank` also grades its penalty by distance past the threshold, so the
objective breakdown accumulates severity rather than multiplying a count by a
weight — a flat multiplication would report a number the objective does not
contain, and the breakdown is what the app shows a human to explain the score.

`PersonPreferenceFit` arrived in the same schema bump and is **not evaluated**.
The conversion layer refuses it as `UNIMPLEMENTED`, the same treatment as
`LOCK_POLICY_MINIMIZE_MOVEMENT`
([ADR-0008](0008-one-solve-mechanism-scope-plus-lock-policy.md)). The app does
not send it yet; the branch exists for any peer that gets ahead of it.
