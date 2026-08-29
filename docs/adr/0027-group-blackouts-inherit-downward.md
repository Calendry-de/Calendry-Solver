# Group blackouts inherit downward, and the query walks up

`GroupVeto` is `LecturerVeto` one entity across, and almost all of it needed no
decisions: an empty message, the windows on `Group.blackouts`, enablement as
tenant policy, a precomputed slot mask, a filter in `is_free`. One thing did.

A blackout is declared on a Group. A Session is attached to a Group. When those
are not the same Group, which of them binds?

## The decision

**A blackout binds the Group it is declared on and that Group's DESCENDANTS.**
Equivalently, from the query side: a Session attached to `g` is blocked by the
windows of `{g} ∪ ancestors(g)`.

`GroupClosure` gained a third table, `ancestry`, for this. It already had the
other two, and both are wrong here:

| expansion | what it is for | what it would do to this rule |
|---|---|---|
| `ancestry` = `{g} ∪ ancestors(g)` | **this rule** | correct |
| `subtree` = `{g} ∪ descendants(g)` | attendance, Group aggregates | a seminar's absence vetoes its cohort's lecture |
| `conflict` = `subtree ∪ ancestors` | double-booking | the same, plus more |

## Why downward

The two directions are not symmetric, because attendance is not symmetric.

A Cohort's lecture is attended by its Seminars (attendance flows down), so if the
Cohort is away, everything inside it is away — a programme suspended for a
placement period takes its cohorts with it. That is the downward half, and it is
the whole point of the feature: a tenant sets the window on the level they
manage, not on every leaf.

Upward would mean the reverse: one Seminar on block placement vetoing the lecture
that every *other* Seminar in the Cohort still attends. That is not a
conservative reading of "this group is away" — it is a small group acquiring
veto power over a large one. The partial-attendance problem it gestures at is
real (should a lecture run when a third of the cohort is away?) but it is a
*preference* about attendance quality, not a hard feasibility rule, and pricing
it would be a different type with a weight.

## Why this is an ADR rather than a comment

**Two of the three candidate expansions are one identifier apart from the right
one, at the single call site that uses it, and `conflict` is the one already in
scope two lines above.** Someone tidying `Problem::build` toward "we already have
a closure, use it" would produce a rule that is wrong in a way no flat fixture
can see: with no hierarchy, all three tables return the same set.

So the guard is a pair of tests over one two-level fixture, in
`crates/core/tests/group_veto.rs`, and the pairing is the mechanism:

* `a_parents_absence_binds_its_child` passes under **all three** expansions. It
  exists to prove the fixture blocks something at all, so its mirror is not
  vacuously green.
* `a_childs_absence_does_not_bind_its_parent` passes under **only** the correct
  one.

Measured against both wrong expansions rather than assumed: `expand_subtree`
fails 4 of the 8 tests in that file, and **`expand_conflict` fails exactly one —
the mirror**. The tempting wrong answer is caught by a single assertion, which is
precisely why it is written down here as well as there.

## Consequences

* The violation detail names the Group that **declared** the window, resolved
  from the ancestry set rather than from `own_groups`. Naming the attached child
  would send a timetabler to a Group with no window on it.
* `Offering.group_veto_slots` is a separate mask from `veto_slots`, and
  `Enforce.group_veto` a separate flag from `Enforce.lecturer_veto`. They are
  separately enableable, and a report has to distinguish "the cohort is on
  placement" from "the lecturer is on leave".
* The app sends the **complement** of what it stores. A tenant records when a
  Group *is* available — a date range inside a Term, which is the shape an
  academic calendar produces — and the assembly inverts it to blacked-out week
  indices. Reusing `Unavailability` rather than adding a positive
  `available_weeks` keeps one convention for absence across `Person` and `Group`;
  the alternative would have put two structurally identical, semantically
  opposite messages next to each other, which is the exact hazard `Preference`
  carries a warning about.
* Generated benchmark instances declare no Group blackouts and leave `GroupVeto`
  off. Drawing windows randomly would change every preset's output, and the
  presets are the baseline; an enabled rule with an empty mask is the
  `lecturer_veto` shape. Realism here belongs behind a gated parameter, as
  `--preferences` is.
