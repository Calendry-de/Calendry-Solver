# The answer accounts for every Session it was given, and new demand never lands before the reference

The app-side investigation of 2026-09-01 ("the vanishing eleven", runs
`01a05ea6`/`01a05eb3`) found the app deleting real, taught Sessions after
every solve, with reason `not_returned_by_solver` — 11 one run, a disjoint 12
the next, forever, while every run honestly reported `converged` under
ADR-0031's contract. The reproducer (`crates/service/tests/past_reference.rs`,
born as `past_reference_repro.rs`) pinned the mechanism as two gaps that feed
each other:

1. **A Session before `reference_slot` came back nowhere.** It classifies as
   `Immovable::Past` (`classify_immovable` tests the reference before
   `is_locked` and scope), becomes fixed occupancy, and satisfies its
   Offering's demand — so it appeared in **neither** `sessions` (the output
   deliberately echoes only what the run placed) **nor** `unplaced_offerings`
   (its Offering is not short). An applier reading absence as orphanhood then
   deletes taught history. Locked Sessions never had this problem only
   because the caller can recognize its own `is_locked` data; past-ness is
   computed solver-side and was invisible.
2. **Nothing stopped a NEW placement landing before the reference.** The core
   never saw the reference at all, so the replacement Session the next run
   invented could itself land in an elapsed week — where the run after
   classified it past, dropped it, and the app deleted it. That is why
   consecutive runs lost *disjoint* sets: each apply seeded the next run's
   losses.

## Decisions

**Every Session the run received comes back either placed or retained.**
`SolverOutput.retained_session_ids` (schema tag `v0.16.1`, on top of the
merged `unplaced-offerings-119` work) lists the id of every immovable
Session the run kept — past, locked, or out of scope under
`LOCK_POLICY_HARD`. The applier's rule becomes one line: a Session is gone
only when it appears in **neither** `sessions` **nor**
`retained_session_ids`. Federation occupancy is excluded
(`FixedSpec::external`): another tenant's booking is not a Session of this
snapshot and "retaining" it would be a claim about data the caller never
sent.

**Retained Sessions are still not echoed as placements.** The existing
contract ("only what this run placed comes back", pinned by
`locked_sessions_are_not_echoed_as_placements`) stands: a `PlacedSession`
entry asserts the run *decided* that placement, and re-reporting the caller's
own data would double-count on apply. The retained list carries identity, not
a decision.

**No new placement may start before the reference.** `ProblemSpec::reference`
carries the run's "now" into the core, and the mask lives in
`SearchState::statically_blocked` — the same occupancy-independent,
monotone-safe gate as the calendar closure, so construction, repair scoring
and the targeted ruin operator all read one definition, and `ruin_blocking`
correctly refuses to hunt blockers for a cell no eviction could open. The
test is the span's **start** against `reference`, mirroring
`classify_immovable` exactly: "too old to move" and "too old to place into"
are one comparison.

**The two `None`s are different, on purpose.** Core `reference: None` — the
default for every fixture and the benchmark generator — masks nothing. The
wire's "no `reference_slot`, or one beyond the term" case already meant
"every Session is past" for classification, so the conversion layer maps it
to one-past-the-last-slot, masking **everything**: a term that is over has
nowhere left to teach. Such a run now surfaces loudly — Sessions retained,
demand in `unplaced_offerings`, termination `stagnated` (ADR-0031) — instead
of quietly placing teaching into elapsed weeks.

## Considered and rejected

* **Having the app compute past-ness itself** and exempt those Sessions from
  orphan deletion. It would have to replicate `resolve_reference`'s
  `lower_bound` semantics and the beyond-term edge exactly — authoring solver
  logic in the app repo, the thing its own conventions forbid, with silent
  drift as the failure mode. The wire carrying the answer is one source of
  truth instead of two implementations that must agree.
* **Marking elapsed weeks closed in the grid.** The calendar closure is
  tenant-calendar semantics at week/day granularity; the reference is
  run-level state at slot granularity, and conflating them would make "why is
  this week closed" unanswerable from the input.

## Consequences

* The proto pin sits on `v0.16.1`, the first tag carrying both
  `unplaced_offerings` and `retained_session_ids`. The app's applier gains
  the two-list rule and should stop deleting on any answer that predates the
  field.
* A mid-term re-solve can no longer relocate future teaching into elapsed
  weeks — capacity genuinely shrinks as a term progresses, and a repair that
  used to "succeed" by hiding a Session in the past now reports the shortfall
  honestly.
* `FixedSpec`/`FixedOccupancy` carry `external: bool`; every constructor
  names it, so a new fixed-occupancy source must decide explicitly whether it
  is the tenant's own Session.
