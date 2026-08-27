# Measure end-to-end impact before optimizing a diagnosed component

A real, correctly-measured inefficiency can still be irrelevant to whole-system
performance if it sits in a small time slice. This has happened three times here,
each a correct measurement that pointed at the wrong work:

* **H1** — repair's enumeration waste was genuinely 149x at large-university
  scale, and fixing it was worth **under 1% of runtime**, because repair sat
  inside a 1% slice.
* **H2** — the retry-all-unplaced multiplier was real and dramatic, but it was a
  **symptom of infeasible instances**, not a defect. It vanished on its own once
  the generator was corrected (ADR-0010, ADR-0011).
* **6b-i** — hoisting the room-independent checks gave 31x on construction, and
  the true bottleneck afterwards turned out to be `evaluate_hard`, **not** the
  room loop the next planned optimization had been scoped to attack.

The corollary that costs the most when skipped: a component's *share* of runtime
decides whether optimizing it matters, and that share **moves** every time
something else is fixed. Re-attribute after every change rather than carrying
forward the previous slice's picture.

**Performance work is closed.** A 27,136-Session university solves in ~350 ms.
The remaining candidate — bitset-intersecting the room-independent axes in
construction — was instrumented rather than assumed: first-fit already exits
after ~26% of the slot space while a bitset computes all of it, so the estimate
was 3–5x on construction and ~1.7x whole-run, for a larger change than either
fix that landed. Against three consecutive overestimates, that did not justify
the work on a case already this fast. Do not reopen it without a new measurement
showing the run is too slow in practice.
