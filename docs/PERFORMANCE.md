# Where a run's time actually goes

Measurements, not decisions. The decisions this record informs are
[ADR-0021](adr/0021-measure-end-to-end-before-optimizing-a-component.md) —
measure end-to-end impact before optimizing a component, and performance work is
closed — and
[ADR-0010](adr/0010-calibrate-on-the-binding-axis.md) / [ADR-0011](adr/0011-a-load-metric-cannot-certify-feasibility.md),
which are why these figures can be trusted at all.

Reproduce with:

```bash
cargo run --release -p calendry-solver-gen --bin bench -- --moves 200000
```

**Release only.** A debug build measures the per-iteration drift assertion rather
than the search. **Move budgets only**, because a wall-clock-terminated run is
not reproducible ([ADR-0006](adr/0006-two-budgets-and-the-limit-of-determinism.md)).

---

## Solution quality

All four presets place **every** Session, and LNS runs properly at every scale.

| preset | placements | iterations | unplaced | aggregate | soft | day_mix |
|---|---|---|---|---|---|---|
| small-school | 1,497 | 92 | 0 | 3 | 1,232 | 225 |
| large-school | 3,167 | 81 | 0 | 22 | 3,379 | 1,050 |
| small-university | 6,760 | 85 | 0 | 60 | 7,164 | 5,195 |
| large-university | 27,136 | 87 | 0 | 454 | 28,800 | 34,210 |

Objective **totals are not comparable** across either of the two semantic changes
that produced these numbers: the virtual-room fix
([ADR-0022](adr/0022-a-virtual-room-is-not-an-exclusive-resource.md)) raised the
aggregate count, and pricing a mixed day
([ADR-0023](adr/0023-onlineonsitesameday-is-priced-not-forbidden.md)) grew
`hard_penalty` itself to bound the new term. The per-term columns are the ones to
compare.

The per-term figures *are* a fingerprint, and were used as one twice. They were
checked unchanged to the digit across the module-shape refactor
(ADR-0016 through ADR-0019), and then checked to match the independently
documented post-change figures after that refactor was merged with the semantic
work — same aggregate, same soft, same day-mix, same violation count. Two
rewrites of the same code meeting on the same numbers is the strongest evidence
available that neither changed behaviour it did not mean to.

## Phase timings

| preset | construct | `evaluate_hard` | LNS | total solve |
|---|---|---|---|---|
| small-school | 2.7 ms (4%) | 0.6 ms (1%) | 70.5 ms (96%) | 73.7 ms |
| large-school | 6.1 ms (9%) | 1.5 ms (2%) | 60.6 ms (89%) | 68.2 ms |
| small-university | 19.9 ms (29%) | 6.0 ms (9%) | 43.5 ms (63%) | 69.5 ms |
| large-university | **119 ms (48%)** | 46.7 ms (19%) | 82.4 ms (33%) | **248 ms** |

**There is no single bottleneck — it is scale-dependent.** At school scale LNS
dominates, but that is the move budget being spent rather than a defect: LNS time
is roughly constant (43–82 ms) across an 18x range of instance size because runs
are budget-bound at 200k moves, so its *share* falls as instances grow. Only at
large-university does construction dominate, and it no longer dominates outright.

Construction got **~46% faster** at large-university (219 ms → 119 ms) purely as
a side effect of pricing the day mix instead of filtering on it: with no per-
candidate day-mix check, the room loop does less work per probe. Nobody set out
to make construction faster here — which is the corollary above, arriving on
schedule.

Re-attribute after every change. These shares move whenever anything else is
fixed, and carrying forward a stale picture is exactly the mistake ADR-0021
records.

---

## What was fixed, and what it was worth

### Construction re-tested the room-independent axes once per Room

Of the six axes only **room occupancy** and **day-mix** (via
virtual-versus-physical) depend on which Room is being tried. Lecturer, group,
person and veto read the slot alone — yet `construct` re-tested them once per
eligible Room at every slot.

`construct` now tests those four **once per slot**, before the room loop. It is a
pure short-circuit: if they reject, no Room could have rescued the slot. Output
was byte-identical — every objective, iteration count and violation count matched
exactly across all four presets.

| preset | before | after | speedup | mean eligible rooms |
|---|---|---|---|---|
| small-school | 31.1 ms | 3.3 ms | **9.4x** | 14 |
| large-school | 108.1 ms | 7.8 ms | **13.8x** | 17 |
| small-university | 560.4 ms | 24.9 ms | **22.5x** | 36 |
| large-university | **7.12 s** | **229 ms** | **31.1x** | 83 |

The estimate on record beforehand was "~60% floor, maybe ~50x". The floor was the
wrong model and badly understated it: 60% was the share of *probes* saved, but the
probes are not equal. A room check is one early-exiting bit test; the
room-independent path scans an attendee list averaging 65 people, and that scan
previously ran once per *free* Room per slot. Speedup tracks
`~0.4 × eligible_rooms` — 0.4 × 83 = 33 against 31.1x measured — which is the
room-occupancy rate deciding how often the expensive path was redundantly
re-entered.

The mask now lives on `Occupant::room_independent_probe`, because the benchmark
harness's construction attribution must use the identical one to report
truthfully. It used to hold a verbatim copy.

### `structural` pair-scanned attendee lists

`evaluate_hard` was 69% of a large-university run, and `structural` was 99.2% of
that. Attribution inside it, by successively disabling parts:

| | cost | share |
|---|---|---|
| attendee intersection, per pair | 458 ms | 72% |
| `format!` of the slot label, per pair | 160 ms | 25% |
| group closure | 6.5 ms | 1% |
| lecturer + room | 11 ms | 2% |
| views + bucketing + loop | 3.8 ms | <1% |

Both dominant costs were **unconditional**: `check_pair` allocated the location
string before checking anything, and ran all four clash searches before consulting
whether any instance covered the pair. Emptying every constraint list changed the
time by **0.2%**, so none of it was reporting.

Fixed by inverting the person axis — per slot, map each attendee to the Sessions
holding them and look for one held twice, instead of asking every pair whether
they intersect — and by making the slot label a `Display` rendered only inside a
real violation message. `structural` **623 ms → 40.4 ms (15.2x)**, output
byte-identical.

### Repair's candidate space is addressed by index

A full enumeration is `slots × eligible_rooms`. Materializing it to keep 512
candidates cost 65% of repair time at large-university scale with 99.4% of the
work discarded. Repair now samples with a virtual partial Fisher–Yates over
`[0, total)`, consuming the RNG in the same sequence, so the same seed still gives
byte-identical output.

---

## Instrumented and deliberately not done

**Bitset-intersecting the room-independent axes in construction.** Measured
rather than assumed: first-fit already exits after ~26% of the slot space while a
bitset computes all of it, so the estimate was 3–5x on construction and ~1.7x
whole-run — a larger change than either fix above, on a run already at 365 ms.
Against three consecutive overestimates (ADR-0021), that did not justify the work.

**Restricting `structural` to immovable pairs.** ~99.75% of its pairwise scan
provably cannot report anything. Not exploited, and the reason is
[ADR-0014](adr/0014-structural-stays-independent-of-occupancy.md): it would make
the authoritative check depend on the correctness of the thing it exists to
verify.

---

## Things that turned out not to be the lever

Both are recorded because each was a *correct* measurement pointing at the wrong
work — the pattern ADR-0021 exists to name.

**H1**, repair's enumeration waste, was genuinely 148x at large-university scale.
Fixing it was worth well under 1% of wall time, because repair sits inside a 1%
slice.

**H2**, the retry-all-unplaced multiplier, was real and dramatic but was a
**symptom of infeasible instances** rather than a defect. It is gone at every
preset size: with 0 unplaced there is nothing to retry, and LNS completes 80–91
iterations everywhere.

---

## The move budget does not buy what it looks like it buys

Measured at large-university, seed 1, varying `--moves` only:

| moves | iterations | aggregate | soft | solve |
|---|---|---|---|---|
| 200k (bench default) | 85 | **455** | 29,181 | 339 ms |
| 1M | 444 | **386** | 25,622 | 627 ms |
| 5M | 2,181 | **219** | 15,093 | 2.28 s |

Construction ends at ~478 violations. The curve is **monotone and still falling
steeply at 5M**; no plateau anywhere in the range. Wall cost scales far better
than linearly — 25x the moves for 6.7x the time — because construction is a fixed
~220 ms and iterations per second *improves* with run length (251 → 955/s).
Per-iteration yield does decay: 0.27 → 0.21 → 0.12 violations removed per
iteration.

Why so few iterations, and why that is the whole story:

* `k = 1 + rng.below(8)` — a ruin touches ~4.5 placements.
* `MAX_CANDIDATES = 512` scored per repaired placement, so ~2,300 moves per
  iteration. **Moves buy candidate breadth, not coverage.**
* 200k moves therefore repairs roughly **380 of 27,136 placements — 1.4% of the
  instance**.
* **`COOLING = 0.999` is per iteration.** At ~86 iterations the temperature is
  still 0.918x initial. Only the 5M run (2,181 iterations, 0.113x) completes
  anything resembling an annealing schedule.

**This is not scale-dependent, which was the surprise.** Every preset lands at
85–88 iterations at 200k moves, because iteration cost is `k × MAX_CANDIDATES`
and is independent of instance size. So the 200k-move budget is an essentially
isothermal walk at *every* scale.

**Recorded as a wrong prediction, deliberately.** Reasoning from the cooling
schedule alone, the expectation before measuring was that extra budget merely
extends a near-zero-temperature hill climb and plateaus. The opposite is true:
extra budget buys the *first actual annealing*. That was not knowable without
running it — which is the whole of
[ADR-0021](adr/0021-measure-end-to-end-before-optimizing-a-component.md).

The cost of using this lever: it reopens a performance envelope slice 6
deliberately closed (7.79 s → 349 ms). **Spending 2.28 s where 349 ms was
celebrated must be a conscious decision, not drift.**

## What `ruin_worst` can see

`ruin_worst` ranks placements by `problem.soft.cost` alone — the unary table. At
large-university soft is 29,181 of an objective of 172,885,956: **0.017%**. So the
arm whose job is "ruin the worst thing" is steering by a rounding error, while the
other two arms are random and related.

LNS does not merely fail to *stumble onto* share breaches; one third of its
selection is actively aimed at the wrong quantity. Pricing a mixed day
([ADR-0023](adr/0023-onlineonsitesameday-is-priced-not-forbidden.md)) added a
second term it cannot see, and among the *tunable* terms its visibility fell from
100% to 45.7%.

The harness prints that ratio every run, so the number moves when the code does
rather than living in a comment. What to do about it is
[ADR-0025](adr/0025-maxonlineshare-is-not-enforced-by-the-search.md).

`PersonPreferenceFit` is the first term added since that was written which moves
the ratio the *right* way, because it is placement-local by construction
([ADR-0026](adr/0026-personpreferencefit-charges-the-unmet-fraction.md)). At
large-university with half the lecturers stating a preference, `ruin_worst`'s
share of the objective goes **0.0055% → 0.0114%**. Still a rounding error; the
point is only that this term does not widen the blind spot.

## `PersonPreferenceFit` costs no measurable time

Measured because the whole representation was chosen to make it true, so a claim
was worth checking rather than asserting. Release, `--seeds 1 --moves 20000`, one
machine, one sitting:

| run | placements | construct | solve | soft |
|---|---|---|---|---|
| large-university, no preferences | 27,136 | 125 ms | 203–225 ms | 29,560 |
| large-university, `--preferences 0.5` | 27,130 | 123 ms | 214 ms | 59,615 |

Inside run-to-run variance on both phases, with the preference term contributing
about half the soft objective. The two instances differ slightly because
generating preferences consumes RNG, so this is not a controlled A/B of one
instance — it is a check that the term does not cost a phase, which is what the
`placement × (day, block)` key was for: the attendee scan happens once per
placement at setup, not once per candidate evaluation. A per-candidate
aggregation over the lecturer set would have put back exactly the scan whose
removal was this project's largest measured win (31× on construction).

**Nothing above this line changes**, because every preset sets
`preference_ratio: 0.0`. `--preferences RATIO` is the only way the rule enters a
generated instance, and the generator gates the RNG draw on that ratio rather
than drawing unconditionally — an unconditional draw shifted every subsequent
draw and silently turned the 27,136-Session `large-university` in the table above
into a 27,134-Session instance reporting the same name.

## Violations after the virtual-room fix

Both measured before and after
[ADR-0022](adr/0022-a-virtual-room-is-not-an-exclusive-resource.md):

| preset | aggregate before → after | soft before → after | unplaced |
|---|---|---|---|
| small-school | 4 → 5 | 1,254 → 1,288 | 0 → 0 |
| large-school | 23–24 → 32–33 | ~3,400 → ~3,470 | 0 → 0 |
| small-university | 60–63 → 62–69 | ~7,200 → ~7,300 | 0 → 0 |
| large-university | **167–180 → 448–461** | ~26,100 → ~29,200 | 0 → 0 |

Structural violations are **unchanged at exactly 80** (14 group + 66 person) at
large-university before and after, and `unplaced` stays 0 everywhere — so the fix
moved nothing it should not have. Objective totals rose 25–49% at school scale and
153–176% at large-university, entirely through the hard penalty applied to the
aggregate count.

Totals are not comparable across the day-mix reclassification either, because
`hard_penalty` itself grew to bound the new term.
