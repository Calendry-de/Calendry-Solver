# Convergence is never declared over unplaced demand

`#120` on the Calendry board reports six consecutive runs of the same instance
— same term, same tenant, ~100 in-scope Offerings, no data changed — placing
between 170 and 208 Sessions, every run reporting `termination_reason:
converged` and zero hard violations. Construction is seed-independent
([`CLAUDE.md`](../../CLAUDE.md)), so all six runs started from the identical
partial solution; only the LNS walk differed. A 38-Session spread between
"converged" runs of one instance is not a capacity ceiling, it is the search
giving up at different depths of the same landscape — which the card correctly
split from the *reporting* gap filed as `#119` (the "zero hard violations"
figure is `ExactFrequency` being an opt-in constraint the tenant had not
configured, not evidence of anything about the search).

## What the investigation ruled out

The card offered two hypotheses. The first — *the objective does not penalize
an unplaced Session heavily enough* — is provably not it. `hard_penalty` is
derived so that one hard count outranks every reachable soft configuration
(`Problem::build`, and the `one_unplaced` bounds tests in `soft.rs`), and any
round that nets one more placed Session has `delta < 0` and is accepted
**unconditionally**, at any temperature. The objective already wants these
Sessions placed more than it wants anything else.

The second hypothesis is the real mechanism, and it is three interlocking
facts, none individually wrong:

1. **Repair never evicts.** A candidate cell occupied by anything scores
   infinite (`is_free` in the evaluator), so a wedged unplaced Session can only
   be placed in a round where ruin already moved its blockers out of the way.
2. **No ruin operator knows what the unplaced Sessions need.** `ruin_random`,
   `ruin_worst` and `ruin_related` all select among *placed* Sessions — by
   chance, by cost, or by sharing a resource with a uniformly-random anchor.
   Freeing the exact `(slot, room)` a wedged Offering could use is left to
   luck, with a disruption budget of at most 8 Sessions per round.
3. **The stagnation counter cannot tell "no improving move exists" from "the
   right combination was never sampled."** It expires after a window scaled
   only by instance size, and declares `"converged"` regardless of whether
   demand remains unplaced and regardless of remaining move budget.

As the easy Sessions get placed, whatever remains is by definition the hardest
— needing an increasingly specific eviction that undirected sampling hits
increasingly rarely — while the patience window stays fixed. Which seed's walk
happens to sample the right combinations before the counter expires is a dice
roll, and 170-vs-208 is that dice roll made visible.

## Decisions

**`"converged"` is reserved for runs whose best solution has `unplaced == 0`.**
While unplaced demand remains and budget remains, hitting the stagnation limit
**escalates** instead of terminating: the ruin-size cap doubles and the
temperature is reheated (at `MIN_TEMPERATURE` acceptance is greedy, and large
rearrangements pass through soft-cost-worse intermediate rounds that greed
rejects). The ladder is finite; a run that exhausts its top level stops with a
**new reason, `"stagnated"`** — the honest answer "I kept demand unplaced and
ran out of ideas, not out of budget." De-escalation back to the base level
happens whenever the best-known unplaced count drops: the now-smaller problem
gets fresh patience at normal intensity. Every decision in this loop is driven
by counters and the seeded RNG, so `"stagnated"` joins `"converged"` and
`"move_budget"` in [ADR-0006](0006-two-budgets-and-the-limit-of-determinism.md)'s
deterministic, byte-reproducible set — and because the ladder is finite, an
unbudgeted call still terminates, which is the reason the stagnation limit
exists at all.

**The rule is scoped to `unplaced` alone, not to all of `hard()`.** The other
hard counts — `aggregate`, the `SameTime` family, `Precedence` — can be
genuinely unsatisfiable by the data, and
[ADR-0025](0025-maxonlineshare-is-not-enforced-by-the-search.md) already
accepts a run succeeding while reporting them. `unplaced` is different in
kind: it is the completeness dimension the product treats as "zero or explain
yourself," and the one whose feasibility sibling runs of the same instance
empirically demonstrate.

**A fourth ruin arm, `ruin_blocking`, is added — and this does not contradict
ADR-0025's "fix `ruin_worst`, do not add a fourth arm."** That rejection was
right because `ruin_worst` had a fixable inconsistency: it scored placements by
`soft` when the objective had grown other terms, and correcting the score made
every placed Session's true cost visible to the existing operator. An unplaced
Session has **no placement to score**. What blocks it is a relation between
its candidate space and the current occupancy — a property of *other*
Sessions' placements that no per-placement cost correction can ever surface,
because the blockers may individually be perfectly cheap. The new arm picks an
unplaced Session, probes its candidate cells, and ruins the cheapest movable
blocker set of the best cell; it supplies information no scoring fix can, which
is the test ADR-0025 implicitly applied. Cells blocked by fixed or locked
occupancy are skipped — a preset never moves to make a count fall, and neither
does anything else immovable.

**Within a round, unplaced Sessions are repaired first, and repair order is
shuffled with the seeded RNG.** Today the removal set is sorted by
`PlacementIdx` and repaired in that order every round, so when two Sessions
contest one freed cell the lower index wins *every iteration, forever* — the
same neighbourhood collapse already fixed once inside `repair_one`'s tie-break
(a documented, observed defect), recurring one level up. Unplaced-first is
aligned with the objective (placing one is worth more than any soft cost a
re-placed Session could recover), and a seeded shuffle within each class keeps
the run reproducible while making the contested-cell race a real one.

**When candidate sampling finds nothing feasible for an unplaced Session,
repair falls back to an exhaustive feasibility scan.** `MAX_CANDIDATES`
sampling happens before feasibility, so all 512 samples can be occupied while a
free cell sits outside the sample — repair then reports "no placement exists"
for a Session that had a home. The fallback is a cheap `is_free` bit-test pass
over the full candidate space, scoring only what it finds, and it runs **only**
for unplaced placements and **only** after the sampled pass came up empty — the
exact situation where a miss is most expensive and the extra scan is worth its
cost. A fallback that still finds nothing is a true statement that the Session
is wedged, which is precisely the signal `ruin_blocking` wants.

## Considered and rejected

* **Committing the good sub-result of a rejected round** (a stuck Session
  placed inside a round that netted worse). It breaks the journal's atomicity
  for a benefit `ruin_blocking` delivers cleanly: the lost opportunity becomes
  reproducible on demand instead of rescued from a rollback.
* **Seeding construction, or random restarts.** Construction is deliberately
  seed-independent; decorrelating runs by randomizing their common starting
  point treats the symptom (variance) while leaving the search equally unable
  to escape whichever basin it starts in. Revisit only if the measures above
  underperform.
* **Weighting `unplaced` above the other hard counts.** The dominance bound
  already guarantees any unplaced-reducing round is accepted unconditionally;
  the counts were never in competition in the way that would require it.

## Consequences

* `termination_reason` gains the value `"stagnated"`. It is a plain string on
  the wire, so no schema change — but the app must not treat an unrecognized
  reason as success, and "stagnated with N unplaced" versus "converged" versus
  "budget exhausted" is exactly the distinction `#119`'s reporting work needs
  to surface to tenants. Coordinate the two.
* An unbudgeted run on a genuinely over-subscribed instance now runs for the
  whole ladder — several stagnation windows at growing ruin sizes — before
  stopping, where it previously stopped after one. That is the intended trade:
  the early exit was cheap because it was wrong.
* Adding operators and shuffles changes RNG consumption, so byte-identical
  outputs shift **across versions**. Within a version, same `(input, seed,
  move budget)` still gives byte-identical output; no cross-version guarantee
  ever existed.
* The happy path — construction places everything, or LNS finishes the job at
  base intensity — is untouched except for one counter and one branch. Preset
  benchmark timings must be re-measured before and after regardless
  ([ADR-0021](0021-measure-end-to-end-before-optimizing-a-component.md)).
