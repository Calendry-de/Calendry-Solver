# Multiple candidates are independent seeded runs, filtered for distance — not a batch RPC

`#4` on the Calendry board asks for several genuinely different schedules to
choose between, rather than one best result per run, and names three
unanswered questions before any code: what makes two candidates different,
where the variation comes from, and how N results reach the app. It also named
a DRAFT wire addition — `SolverOutput.candidates` (`SolverCandidate` wrapping
`sessions`/`hard_violations`/`objective`) — staged "only so the design
conversation has a concrete shape to react to," and marked NOT COMMITTED at
the time the card was written (2026-08-29).

**Correction found while writing this ADR: it is committed.**
`855c145` ("Stage P0 wire surface … draft candidates") is an ancestor of the
published `v0.11.0` tag — the field shipped in the npm package alongside
several other P0 wire additions in the same batched release, despite its own
proto comment still saying DRAFT — NOT COMMITTED. That comment is now simply
wrong, and is worth fixing in `calendry-proto` independent of this ADR: a
proto comment asserting something about its own repo's git history is exactly
the kind of claim that goes stale silently.

This ADR answers the three questions, and its answer means **no USE for that
field** — not that it should be removed. Removing a published field is a
breaking wire change, which needs its own justification and is not one this
ADR provides. `SolverOutput.candidates` stays in the schema, permanently
unpopulated: every existing caller already treats it as absent (an empty
repeated field), so leaving it inert costs nothing and undoing the release is
the larger, unforced move. No new `calendry-proto` or `calendry-solver` change
is required for a first version of this feature.

## Where the variation comes from: independent seeds, never a diversity term

The card names three options: different seeds, different soft weightings, or
an explicit diversity term in the search. It also names the deciding fact
itself: a diversity term is "the only one that *guarantees* difference, and it
is also the only one that changes the objective" — which means it is the only
one that needs new solver work, new evaluator interactions, and a new argument
for why `(input, seed, move budget) → byte-identical` (solver ADR-0006) still
holds with a term in the objective whose whole job is to depend on *other
candidates already produced in the same batch*. That is a real solver-search
question, and nobody has asked for the diversity such a term buys — a handful
of independently-seeded runs already gives more variation than a reviewer can
usefully compare.

**So: N candidates are N ordinary runs of the same `(input, scope)` at N
different seeds**, run and terminated exactly as any run is today. Nothing
about the search changes. Nothing about the objective changes. Each candidate
individually keeps the existing determinism guarantee, unconditionally,
because each *is* an existing, unmodified kind of run.

## What makes two candidates different: a computed distance, applied to a larger pool than is shown

Different seeds do not guarantee different results — near-identical outcomes
are common on an under-constrained instance, and the card is right that a
near-duplicate is worse than one answer: it costs the reviewer time for no real
choice.

The fix is not to make the search produce different results (the diversity-term
option, rejected above). It is to **solve more candidates than are shown, and
select for distance afterward**:

1. Solve a POOL of *k* seeds (k larger than the number ultimately displayed —
   4–8 is a reasonable starting point, tunable without any interface change).
2. Define the distance between two candidates as the count of Sessions whose
   `(day, block, room)` differs between them. Cheap — O(sessions) — and legible
   to a reviewer as "N sessions moved," which is already the shape
   `refreshViolations`/the review UI thinks in.
3. Greedily select a small display set (the classic farthest-point / max-min
   selection): start from the best-objective candidate, then repeatedly add
   whichever remaining candidate has the largest minimum distance to everything
   already selected, until the display count is reached.
4. A candidate closer than some minimum distance to every already-selected one
   is dropped from consideration entirely, rather than shown as a near-duplicate
   with a worse objective.

This is pure post-processing over already-completed, already-deterministic runs.
It needs no new solver internals — no change to `Problem`, `Trial`, `Objective`,
or any evaluator — because it operates on the OUTPUT sessions of N ordinary
solves, not on anything inside a single search.

## How N reaches the app: it doesn't need a batch RPC, because the app already has all N results

This is the part that changes the card's own scoping. `StartRun` and
`GetStatus` are unmodified: the app starts N ordinary runs (same input, same
scope, N different seeds — recorded, see below), polls each with the existing
single-run `GetStatus`, and performs the distance computation and the
farthest-point selection **in `calendry`**, once every run in the pool has
reached a terminal state. Nothing added to `SolverOutput`. Nothing added to
`SolverInput`. `SolverCandidate` is unneeded — this design produces N candidates as N
ordinary `StartRun`/`GetStatus` round-trips, never by populating one run's
`candidates` array — but it is already published and does not need reverting;
see the correction above.

Two things this DOES require, and both are app-side, not solver-side:

- **A batch grouping.** The app needs to remember "these N `solver_run` rows
  are one candidate request" so the review screen can wait for the whole pool
  and so re-requesting "the same candidates" is well-defined — a
  `solver_run_batch` row (or a nullable `batch_id` + `seed_index` column on
  `solver_run`) naming the base input/scope and the exact seed list used.
  Reproducibility then falls out for free: the SAME seed list against the SAME
  `(input, scope)` reproduces the SAME pool, because each member run already
  has that guarantee individually, and the existing idempotency key
  (`<inputHash>:<scopeHash>:<seed>`) needs no change at all — a batch is simply
  N of the keys that already exist, not a new kind of key.
- **A comparison screen**, genuinely different from the existing one. Today's
  review compares one proposal against the current schedule. This compares N
  proposals against EACH OTHER, with a legible per-candidate summary — sessions
  moved from the pool's best-objective candidate, and ideally which Groups or
  Offerings the difference concentrates in, since "12 sessions moved, scattered
  everywhere" and "12 sessions moved, all Group A's mornings" are very
  different choices to present. Applying a chosen candidate reuses the existing
  single-Generation apply path unchanged — a chosen candidate is not a new kind
  of object once chosen, it is an ordinary Generation like any other run's
  result.

## Consequences

**Cost is N× a single solve**, run sequentially or in parallel depending on
what the deployment can afford — this ADR does not decide that, since it is an
ops/scheduling question with no bearing on correctness. A tenant asking for 3
displayed candidates from a pool of 6 pays for 6 ordinary solves.

**The selection algorithm needs its own home.** Distance computation and
farthest-point selection are pure functions over completed session sets with
no solver-internal state, so they belong in `calendry` (likely alongside
`refreshViolations`, which already reasons about Sessions at this
granularity), not in `calendry-solver`. This ADR deliberately does not stage
anything in this repo for that reason — there is nothing here to build.

**If diversity ever needs to be stronger than what seed variation plus
selection can produce** — every seed in the pool landing within the minimum
distance of each other on a heavily-constrained instance — that is the
signal to revisit the diversity-term option this ADR declines, and it is a
new ADR when it happens, not a silent addition to this one.
