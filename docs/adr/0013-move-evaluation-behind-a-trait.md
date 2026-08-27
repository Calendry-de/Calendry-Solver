# Move evaluation sits behind a trait, for a future GPU backend

`MoveEvaluator` is **batched by construction**: a per-move `score(&Move) -> Score`
signature would make a GPU implementation pointless, since the entire value there
is scoring thousands of LNS candidate moves per dispatch. Keeping the batch in
the signature means a future backend plugs in without the search changing.

That backend is **not being built**. The point is only that the architecture must
not foreclose it.

The seam is a **parameter on `solve_with`**, not a local inside `solve`. It was
originally a hardcoded `let evaluator = CpuEvaluator;`, which meant swapping a
backend required editing the search module — exactly what a seam is supposed to
make unnecessary, so the ADR was satisfied on paper only. `Halt`, a few lines
away, is the shape it now copies: parameter on `solve`, three real adapters.

## Consequences

`solve` is kept as a convenience wrapper defaulting to `CpuEvaluator`, so
existing callers are unaffected. The parameter is generic rather than `&dyn`, so
the hot loop dispatches statically.
