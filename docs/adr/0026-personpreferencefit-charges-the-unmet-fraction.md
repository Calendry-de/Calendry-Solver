# `PersonPreferenceFit` charges the unmet fraction, and it is not a `SoftParams` variant

The design record for this feature (`per-person-preferences-design.md`, in the
Nuxt repo) specifies the per-placement cost as

```
cost = tenant_weight × ( Σ_{p∈P} m(p) × fit(p) ) / |P|      // fit = satisfaction
```

and the schema comment calls the type a reward: "Reward placements landing in a
Person's stated preferred days/blocks." Read literally, in a minimizer, those
two sentences disagree — the formula charges *most* for the placement that suits
everybody perfectly. Both decisions below settle that, and one of them
contradicts the design record's mechanism section.

## The term is a penalty on `1 - fit`, not a reward for `fit`

Implemented as `weight × mean( m(p) × (1 - fit(p)) )`. The intent in the design
record is unambiguous — preferred placements should be cheaper — and the sign is
the only thing that had to be chosen. A penalty on the unmet fraction and a
reward for the met one order the candidates identically for a fixed placement,
so the choice is decided entirely by what each does to the machinery around it.

A negative soft term would have moved four things at once:

* **`hard_penalty` stops bounding anything.** It is derived as
  `sum(weights) × placements + 1` precisely so that one unplaced Session
  outranks every reachable soft configuration. A term that can go negative makes
  "reachable soft configuration" unbounded below rather than above, and the
  argument has to be re-made from scratch.
* **`Objective::total() == 0` stops meaning "nothing left to fix".** The search
  early-exits on it (`search.rs`), and a reward can reach zero by cancelling a
  real cost.
* **`ruin_worst` stops ranking the worst.** It sorts placements by descending
  soft cost; rewarded placements sort below zero-cost ones, so the operator
  would preferentially ruin *satisfied* placements.
* **The breakdown reports a negative `weighted`**, which the app renders as part
  of an explanation of a score.

None of that is worth an ordering that is otherwise identical. The two forms
differ by `weight × mean(m(p))` per placement — not a constant across
placements, since `|P|` and the multipliers vary — so this is a genuine change
of number, not a shift, and it is recorded rather than folded in silently.

Note the consequence for the counted set: a lecturer who stated nothing must be
**excluded from `P`**, not included with `fit = 0`. Under the reward reading
those two are the same; under the penalty reading, including them charges every
placement in the tenant forever. The test
`a_lecturer_who_stated_nothing_is_not_counted` is red against exactly that.

The same reasoning decides what happens to a stored value this run's grid has no
slot for — a `block 9` on an 8-block grid, which is legitimate data because the
app validates preferences against the tenant's *widest* grid. It is **dropped**,
matching `MinimizeBlockUsage`, where a stale index "simply never matches". Kept,
it would do the opposite of inert: an axis that can never be satisfied charges
the person at every slot in the grid with no placement able to fix it.

## It gets its own instance list, not a `SoftParams` variant

The design record says "`SoftParams` gains a `PersonPreferenceFit { roles }`
variant". It does not, and the reason is already written down one field above
where it would have gone:

> Its own list rather than `soft` below, because `SoftModel` is a precomputed
> `(slot, room)` table and a mixed day depends on what else is already placed
> — see `DayMixInstance`.

`SoftModel` is keyed by `(profile, slot, room)`, where a profile is the set of
instances applying to one tenant `kind`. A preference cost depends on **who
leads this placement**, which varies per placement rather than per kind. Keying
it into the profile dimension means one profile per distinct preference
signature — potentially one per placement — and the table stops being small.

Folded in anyway, the variant would have needed `SoftParams::applies` and
`::severity` to return `false`/`0.0` for it, so the type would sit in the
catalogue of slot-keyed predicates as two dead arms that a reader has to know
are dead. `ConstraintSet::person_preference_fit` plus `PreferenceModel` costs
one field and one module and states the shape in the type.

**What the design record got right, and this keeps, is the objective field.**
The cost goes into `Objective::soft` and is delta-accumulated, *unlike*
`day_mix_cost`. The criterion is the one `day_mix_cost`'s own comment states:
`soft` is a per-placement unary cost the search accumulates; the day-mix field
is read whole off counters because a mixed cell belongs to no single placement.
A preference cost belongs to exactly one placement. So it is `soft` by that
criterion, which buys the existing drift assertion, `ruin_worst` visibility, and
no new field for the breakdown and the search to disagree in.

## `hard_penalty` must count the multiplier, not the weight

`sum(weights) × placements` is the wrong bound for this term, and short in the
one direction that matters. A Person may carry a `weight_multiplier`, so one
placement can cost up to `weight × MAX_WEIGHT_MULTIPLIER`; summing raw weights
would leave a heavily-preferred schedule able to outrank a hole in the
timetable.

`PreferenceModel::max_cost_per_placement()` supplies `weight × 2.0`, and the
important property is *what it does not read*: it is computed from the
constraint configuration alone, never from how many lecturers have an override
on file. That is what keeps a tenant-editable column out of a number the
lexicographic ordering depends on — the same discipline that made
`MinimizeRoomRank::severity` normalise to `0.0..=1.0` so a graded rule's maximum
contribution still equals its configured weight.

The chain the design record establishes holds because of it: lecturers-only
scope and raw per-placement accrual make the term placement-local, which gives
it a per-placement quantity to normalise, which gives the bounded per-person
override a ceiling it cannot escape. Reversing any link breaks the last one.

## The mean over the product, and why the fixture has to be lopsided

`mean(m × unmet)`, not `sum` and not `mean(m) × mean(unmet)`.

Sum is bounded by `max_multiplier × |P|`, which puts an instance-data quantity
back into the ceiling above — a tenant could raise this type's contribution
arbitrarily by raising `required_lecturer_count`, with no weight change and no
warning.

The separated product is bounded too, so **a bound check does not separate it
from the correct form**; worth saying plainly, because "both are bounded" is
exactly the observation that lets someone pick the wrong one and still pass a
check. What decides it is attribution: it applies the average multiplier to the
average fit, so the lecturer with a 2.0 multiplier inflates the cost even on the
day that suits them perfectly.

A fixture with equal multipliers or equal fits makes all three forms agree, so
`two_lecturers_with_opposing_preferences` has both differing — multipliers
`0.5`/`2.0`, and satisfaction that swaps between the two days. The three forms
then disagree in *shape*, not only magnitude: the separated form prices both
days identically and so expresses no preference between them at all. Each wrong
form was implemented and confirmed to turn the tests red.

## A non-empty `roles` is refused

The wire carries `PersonPreferenceFit.roles`, "which role_tags' preferences are
counted", and empty means lecturers only. Only the empty case is implemented; a
non-empty one returns `UNIMPLEMENTED` rather than being honoured approximately.

Widening the counted set is not a small mistake to make silently: a Session's
attendee set is its lecturers plus every member of every attached Group's
descendant closure, averaging ~65 people at benchmark scale, so counting
attendees turns "this tutor prefers mornings" into an unweighted vote a
200-student cohort wins. The precedent for a scoping axis the receiver cannot
honour is the app's offering-scoped constraint rows, which are *skipped* rather
than degraded to unscoped, because degrading them would silently widen the rule.

## Consequences

`ruin_worst`'s blind spot narrows rather than widens for once: at
`large-university` with half the lecturers stating a preference, the share of
the objective it can see goes from 0.0055% to 0.0114%. Still a rounding error,
for the reasons in [ADR-0025](0025-maxonlineshare-is-not-enforced-by-the-search.md)
— but this is the first term added since that was recorded which the operator
can actually rank.

Solve time is unchanged at that scale (213 ms against 203–225 ms across preset
runs, inside run-to-run variance), which is the point of the
`placement × (day, block)` key: the attendee scan happens once per placement at
setup rather than once per candidate evaluation. The naive `placement × slot`
table would have been ~25 M entries against 1.1 M for the same information,
because a preference has no week axis and that table would store each
`(day, block)` value once per week of the term.

**The precomputation is only valid while a placement's lecturer set is fixed
before the search starts.** It is, because genuine lecturer-*pool* selection is
unimplemented — a real pool returns `UNIMPLEMENTED`. If pool selection lands,
the set becomes a decision variable, the mean can no longer be precomputed per
placement, and this key is wrong. That is a coupling invisible from the pool
side, and the failure would be silent: a stale mean over whichever lecturers the
Offering happened to list first, still bounded, still plausible, quietly pricing
the wrong people's preferences. The module docs say so where the table is built.

Benchmark presets set `preference_ratio: 0.0`, so nothing in
[`PERFORMANCE.md`](../PERFORMANCE.md) changes; `--preferences RATIO` is the only
way the rule enters a generated instance. The generator gates the RNG draw on
that ratio rather than drawing unconditionally — an unconditional draw shifted
every subsequent one and turned the documented 27,136-Session
`large-university` into a 27,134-Session instance reporting the same name.
