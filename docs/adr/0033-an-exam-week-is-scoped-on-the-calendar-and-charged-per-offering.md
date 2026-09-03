# An exam week is scoped on the calendar and charged per Offering

A calendar period is term-global. `Week.kind` is one enum on one week, so two
cohorts sitting their exams in different weeks is unsayable (Calendry #126). Of
that ticket's three sub-asks, two are app-only — `Group.blackouts` plus
`GroupVeto` already covers per-group *closure* (ADR-0027), and the exam-request
flow never reaches the solver at all. This is the third, and the only one that
does.

Two decisions, and they are independent. One is where the scope is *declared*.
The other is what a group-dependent predicate does to the cost model — and that
is the expensive one, which the ticket's own estimate missed.

## The decision

**`Week.exam_group_ids` — which Groups this week is an exam week *for*. Empty
means every Group.** And **`MinimizeExamWeek` leaves `SoftParams`**, becoming a
per-Offering formula on `Problem` like the rest of the near-miss family.

## Why a narrowing of `Week`, and not a second calendar

The rejected alternative was a `GroupExamWindow` repeated message plus a
group-scoped constraint type. It is more uniform with how `GroupVeto` and the
movement overrides are shaped, and it looks like the safer choice.

It is not, because **the axis being scoped is not the Group — it is the week, and
the week already has exactly one home.** A second message does not move
authority over which weeks are exam weeks out of the calendar; it *duplicates*
it. `Week.kind = WEEK_KIND_EXAM` and a `GroupExamWindow` row can then disagree,
with nothing on the wire to say which wins. That is the redundancy ADR-0028
refuses on the relation side, and the one ADR-0027's own consequences refuse when
they reject a positive `available_weeks` in favour of reusing `Unavailability`.

A narrowing costs one field. A second home costs a message, a conflict-resolution
rule, and a reader who has to know both.

Three conjunctive axes on one message — the week, its kind, its audience — is
the shape `Unavailability` already has, and `crates/core/tests/mid_week_absence.rs`
exists to defend.

### `SlotFlags` does not change, and that was the fear

The ticket conceded that `week_kind` "stops being a scalar for the Exam case" as
the price of its own recommendation. It is not the price. `week_kind` has exactly
two readers — `SlotFlags::is_closed` and the `MinimizeExamWeek` arm of
`SoftParams::applies` — and `is_closed` does not consult the exam case at all.

So the narrowing lives on the Offering instead. `week_kind` stays a scalar,
`Exam` stays *open* rather than closed, and no `(slot, Group)` pair is ever
addressed: the slot table remains the shared address space that makes conflict
detection a table lookup. The property the ticket wanted protected is protected
more thoroughly than the ticket thought possible, which is an argument *for* this
option rather than a cost of it.

## Why it leaves `SoftParams`

The ticket expected a contained evaluator change, on the grounds that
`MinimizeExamWeek` already has the placement's groups. It does not:

```rust
pub fn applies(&self, f: &SlotFlags, room: &Room) -> bool
```

A slot's flags and a Room. That signature is deliberate — `applies` is the single
predicate shared by the cost table and the reported breakdown, so the fast path
and the counts cannot disagree — and it is exactly the signature a group-scoped
rule cannot have.

So `MinimizeExamWeek` joins `MinimizeCapacityWaste`, `MinimizeBreakSpanning` and
`MinimizeSpecializedRoomUse`: its own instance list on `ConstraintSet`, a
per-Offering precomputation in `Problem::build`, a plain formula on `Problem`,
and a ceiling folded into `hard_penalty`. This is ADR-0026's move for
`PersonPreferenceFit`, one axis over.

**The alternative was to keep the table and widen its key** from `kind` to
`(kind, exam-scope class)`, which is genuinely the smaller conceptual change. It
was measured rather than argued away:

| | |
|---|---|
| `large-university` grid | 1008 slots × 140 rooms = **1.13 MB** per table |
| cohorts in that preset | **80** |
| worst case | ~80 scope classes × the existing per-kind profiles ≈ **90 MB** of tables, ~80× the table build |
| total solve today | **248 ms** |

A profile is a per-*kind* object; this predicate is per-*Offering*. The two
partitions do not agree, and forcing them to costs a phase for a term (ADR-0021).
ADR-0026 refused the identical move for the identical reason.

An Offering's Group set is fixed before the search starts — unlike a lecturer
pool — so the whole decision precomputes into `Offering::exam_week_slots`, a slot
mask sitting beside `group_veto_slots`, and `Problem::exam_week_cost` stays a bit
test and a float read.

## The query walks up, for ADR-0027's reason one axis over

An exam period declared on a Group binds that Group and its **descendants**: a
programme's exam fortnight covers its cohorts, which is the point of scoping at
all — a tenant sets the period on the level they manage, not on every seminar
leaf. From the query side that is the ancestor chain, so
`GroupClosure::expand_ancestry`, and neither of the other two tables.
`expand_subtree` points the wrong way; `expand_conflict` is the ancestors plus
every descendant.

ADR-0027 caught the same trap for `GroupVeto`, and the guard is the same shape —
a mirrored pair of tests over one two-level fixture, because on a flat hierarchy
all three expansions agree.

**What is new here is that `invert` makes the wrong answer worse than
over-blocking.** For a veto, `expand_conflict` over-blocks: bad, but conservative
in direction. Under `MinimizeExamWeek { invert: true }` it hands the whole
cohort's lecture the *seminar's* exam period as its own, and the search then
actively pulls that lecture into a week it was told to avoid. The wrong expansion
stops being over-cautious and starts steering, so it earns a third test rather
than a comment —
`an_inverted_rule_never_pulls_a_cohort_lecture_into_a_seminars_exam_week`, on a
teaching-then-exam grid so a wrong expansion has to *make* a move to fail it.

## `invert` reads the other side of one mask

The mask is per Offering; `invert` is per instance. The flag picks which side of
the mask is charged, which settles the three cases the ticket left open:

* **Cohorts that disagree — UNION.** An Offering serving `A` and `B` carries both
  cohorts' exam weeks: charged in both under `invert: false` (a joint lecture in
  `A`'s exam week collides with `A`'s exams), free in either under `invert: true`
  (a joint exam may legitimately sit in either period). Intersection is the
  alternative and it is wrong — `{12} ∩ {14}` is empty, so a joint Offering would
  have no exam period at all and an inverted rule would charge it uniformly: a
  constant that steers nothing while inflating the objective and the breakdown.
* **No Groups at all.** Matches no *scoped* exam week, and is still charged by an
  *unscoped* one. An all-staff meeting sits in nobody's exam period, but a
  term-global exam period is still term-global.
* **No Groups, and `invert`.** The mask is empty, so it is charged at every slot.
  **Deliberately not special-cased**, and this needs writing down because the
  empty-mask shortcut is right next door: `charged_specialized_rooms_for` returns
  an empty mask *precisely so* its term becomes unreachable rather than free.
  Copying that here makes the inverted direction silently cost nothing. Two
  reasons to charge instead — exempting on an empty mask would make one
  Offering's cost depend on whether some *other* week carries a scope list, which
  no other term in the objective does; and it is already today's behaviour, since
  an `invert: true` instance over a calendar with no exam weeks charges every
  placement everywhere. One rule, not two.

Because a tenant can instantiate both directions at once — ADR-0024 names that
hazard, and the flag reduces it to one type without removing it — an Offering
carries **two** charges, summed by direction, and one placement pays exactly one
of them. The old table summed both as well, so no number moves.

## Refusals

Two, and both because inert would be a **silent widening**:

* **An unknown group id** in `exam_group_ids` — the existing
  `ConvertError::UnknownGroup`, with its context naming
  `calendar.weeks[N].exam_group_ids`. Dropping it narrows the scope; dropping the
  *only* id widens it from "an exam week for cohort A" to "an exam week for the
  institution".
* **A non-empty `exam_group_ids` on a week whose kind is not EXAM** — the new
  `ConvertError::ExamGroupsOnNonExamWeek`. It could only ever be inert, and inert
  reads as "no exam period here", so ordinary teaching lands on top of the exams
  the scope was sent to protect and the run reports nothing wrong. Structurally
  the `FootprintOnVirtualRoom` refusal (ADR-0022's third addendum), for the same
  reason. This is *not* the `MinimizeBlockUsage` stale-index case, which is inert
  because a tenant's grid can shrink under its own configuration: a week's kind
  and its exam scope are written by one app pass from the same calendar rows, so
  a mismatch is an assembly bug, not drift.

**Not** refused: an empty list on an exam week — that is the fail-open convention
*and* the wire default, so every peer on schema v0.17.0 or earlier lands there —
and a scope naming a Group nobody attends, where the id resolves so nothing
widens; it simply matches nothing, and the solver tolerates inconsequential
input.

## Consequences

* **The compatibility theorem is the acceptance criterion.** With no scope sent,
  `exam_week_slots` is exactly the `week_kind == Exam` slot set, and
  `exam_week_cost` reduces term by term to what the table charged. Every
  preset's `soft` column in `docs/PERFORMANCE.md` must be unchanged, and two
  tests assert the theorem directly — one on the objective, one on
  `hard_penalty`.
* **`hard_penalty` gains a term.** `exam_week_weight * placements` replaces the
  contribution this type used to make through `SoftModel::total_weight`. Without
  it the bound would **shrink silently** when the type left the soft table, and a
  soft preference would gain ground on a hole in the timetable. The
  per-placement ceiling is `max(charge_in, charge_out) ≤ exam_week_weight`, so
  the lexicographic ordering still holds.
* **The breakdown gains a branch.** `soft_breakdown` chains day-mix and
  preference separately *because they are not in `problem.soft`*; this is the
  third, and `ConstraintType` gains a `MinimizeExamWeek` variant carrying the
  unchanged wire string. Without it the type would move the score invisibly,
  which ADR-0024 names as the failure to avoid. Worth noting for later: the rest
  of the near-miss family **is** currently absent from the breakdown for exactly
  this reason, and that is a separate defect this ADR does not fix.
* **The scope lands on `ProblemSpec`, not on `WeekSpec`.** `slots` imports only
  `SlotIdx` and is deliberately the group-free coordinate system, and the
  `SlotTable` is built before the `GroupIdx` space exists. `ProblemSpec` is where
  every other `GroupIdx`-keyed policy already lands, and every fixture spreads
  `..ProblemSpec::new(slots)`, so no existing construction site changes.
* **The generator declares no scope.** ADR-0027's stance, unchanged: drawing
  scopes at random would move every preset's baseline, and the presets *are* the
  baseline. Realism belongs behind a gated parameter, as `--preferences` is.
* **One stale schema comment goes with it.** `constraints.proto` said of
  `MinimizeExamWeek.invert`: "PROTO ONLY. The solver does not yet read this
  field." It has been read since the field landed, and
  `inverted_minimize_exam_week_steers_into_the_exam_week` pins that it actively
  *moves* a Session. The new field's doc comment would otherwise sit directly
  beneath a sentence a reader will believe.
* **An old solver against a new app degrades gracefully but not safely.** An
  unknown field is ignored, so exam weeks stay term-global — today's behaviour,
  and harmless under `invert: false`. Under `invert: true` it attracts *every*
  cohort's exams into *every* exam week. The app must gate the feature on the
  schema pin rather than treating omission as benign.
