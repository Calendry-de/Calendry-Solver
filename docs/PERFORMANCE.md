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

| preset | placements | iterations | unplaced | objective |
|---|---|---|---|---|
| small-school | 1,497 | 91 | 0 | 85,090 |
| large-school | 3,167 | 84 | 0 | 1,067,445 |
| small-university | 6,760 | 89 | 0 | 5,685,722 |
| large-university | 27,136 | 80 | 0 | 68,409,047 |

The objectives are a fingerprint as much as a figure: they were checked
unchanged, to the digit, across the module-shape refactor recorded in
ADR-0016 through ADR-0019.

## Phase timings

| preset | construct | `evaluate_hard` | LNS | total solve |
|---|---|---|---|---|
| small-school | 3.0 ms (5%) | 0.8 ms (1%) | 59.7 ms (94%) | 63.5 ms |
| large-school | 7.2 ms (11%) | 1.4 ms (2%) | 57.8 ms (87%) | 66.4 ms |
| small-university | 24.8 ms (35%) | 6.3 ms (9%) | 40.4 ms (57%) | 71.5 ms |
| large-university | **219 ms (60%)** | 45.5 ms (12%) | 100 ms (27%) | **365 ms** |

**There is no single bottleneck — it is scale-dependent.** At school scale LNS
dominates, but that is the move budget being spent rather than a defect: LNS time
is roughly constant (40–60 ms) across an 18x range of instance size because runs
are budget-bound at 200k moves, so its *share* falls as instances grow. Only at
large-university does construction dominate.

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
