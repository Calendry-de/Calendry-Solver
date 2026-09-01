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

## `MinimizeSpecializedRoomUse` is a SECOND room type, and why that is not a violation

A request to "keep the lab and the computer room free for the teaching that
needs them" looks like this ADR's own example already answered it:
`MinimizeRoomRank` exists precisely to keep the best rooms free, and adding a
second type that also steers room choice is the multiplication this file
argues against. It was checked against that rule rather than waved past it, and
the rule's own test — *is this a second DIRECTION of one axis, or a second
axis?* — comes out on the far side.

**`rank` is ordinal desirability; `is_specialized` is functional scarcity.**
They are not the same claim about a Room, and a real tenant needs both at once:

- A lecture hall is premium and unspecialized — spare it, or fill it, per the
  `invert` flag.
- A computer lab is specialized and entirely ordinary as a room — nobody wants
  it "because it is nice", they want it because it has the computers.
- A tenant wanting *"spare the lab but prefer the auditorium"* has to say both,
  and on one ordinal scale that is unsayable: `invert` is a property of the
  instance, so one rank axis admits exactly one direction at a time.

Worse than unsayable, encoding a lab as high-rank is actively wrong in the
inverted direction: `MinimizeRoomRank { invert: true }` means *prefer the
premium rooms*, so it would pull ordinary teaching straight **into** the lab —
the opposite of the request, produced by the workaround for it.

**The decisive difference is the exemption, which rank cannot express at all.**
The rule is not "avoid this Room"; it is "avoid this Room *unless you need what
it has*". A programming class belongs in the computer lab, and charging it
would price a choice it never had — at best noise on the objective, at worst
pressure out of the only Room that suits it. That exemption reads the
Offering's `required_room_features` against the Room's `feature_tags`, which is
a per-(Offering, Room) question. `MinimizeRoomRank` is kind-scoped and reads
neither, so it cannot distinguish the class that needs the lab from the class
that merely landed in it — the two are the same kind at most tenants.

So this is a new axis, and the flags-not-types rule does not apply. What it
DOES still apply to is the shape of the new type, which is why the type carries
no direction flag of its own: there is no coherent "prefer specialized rooms"
policy to invert toward. A tenant who wants a Room used rather than spared says
that with `rank` and `invert`, on the axis that means it.

### What it is NOT: an inference from how richly a Room is tagged

The rejected alternative was to need no new field at all — penalize any Room
carrying `feature_tags` the Offering does not require, the exact analogue of
`MinimizeCapacityWaste` penalizing a Room larger than `min_capacity`. It is
self-exempting by construction and elegant on paper.

It was rejected because it makes the penalty track **how thoroughly the tenant
filled in `feature_tags`** rather than which Rooms are actually scarce. A room
tagged `whiteboard` would be penalized like a room tagged `computers`, and a
genuinely scarce lab that nobody got round to tagging would be free. Scarcity
is a claim the tenant should make deliberately, not one inferred from data
entry — the same reason `Room.site` is an explicit tag rather than derived, and
the same failure mode ADR-0026 records for letting a tenant-editable column
leak into the objective.

### Flat, not graded, and that is a deliberate departure from its neighbours

`MinimizeRoomRank` grades by distance past its threshold and
`MinimizeCapacityWaste` by a saturating ratio, both because their inputs are
ordinal and unbounded. `is_specialized` is a boolean: there is no "how
specialized", so there is nothing to grade, and inventing a gradient would mean
inventing a severity the data does not contain.

It is also charged at most **once per placement**, not once per specialized
Room a multi-Room Session occupies. That keeps one placement's ceiling exactly
the summed instance weight, so `hard_penalty`'s "each term costs at most its
own weight per placement" bound stays exact rather than needing widening by
`MAX_ROOMS_PER_SESSION` for a case that barely exists.

### It prices, it never filters

Marking a Room specialized changes no Room's ELIGIBILITY. An Offering requiring
nothing is still eligible for the lab — which is the whole reason a soft term
is needed to steer it away, and what guarantees the lab is still used when it
is the only Room that fits. A hard filter here would turn a preference into
infeasibility the moment a tenant marked one room too many.

### Where it lives, and why not in `SoftModel`

Outside the `(kind-profile, slot, room)` cost table, alongside
`MinimizeCapacityWaste` and `MinimizeBreakSpanning`. The table is keyed by a
profile — the instance set applying to one tenant `kind` — and the exemption is
per-**Offering**: two Offerings of one kind, sharing one profile and therefore
one table row, routinely differ on whether they require the lab's features. The
table cannot express it, the same wall `PersonPreferenceFit` hit (ADR-0026).

Instead the entire decision is precomputed per Offering into
`Offering::charged_specialized_rooms` — which Rooms are specialized, whether
this Offering is exempt from each, and whether any instance covers its kind at
all — so `Problem::specialized_room_cost` is a bit test and a float read. The
exemption is a `Vec<String>` intersection, and answering it inside `score_one`
would put string comparison in the innermost loop the search has.
