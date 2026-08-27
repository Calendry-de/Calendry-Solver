# Fourteen predefined constraint types, each a compiled evaluator; no expression DSL

Constraints are configured by choosing a predefined type and filling in typed
parameters. Each type has one compiled evaluator function reading that type's own
parameters. **There is no interpreter and no free-form expression language:
tenant-supplied logic never executes.**

The alternative — a rule DSL — would put arbitrary tenant input on the hot path
of a multi-tenant service, make every constraint's cost unbounded and
unpredictable, and turn a scheduling engine into a language runtime with the
security surface that implies.

**Hard-versus-soft is a property of the type**, compiled into the evaluator,
never a per-tenant configuration field. Each type also declares which Session
kinds it applies to, because a tenant-defined kind may have no Group at all.

## Consequences

Adding a constraint type is a code change in `crates/core/src/constraints.rs`,
by design. `build_constraints` in the conversion layer matches exhaustively with
no `_ =>` arm, so a new type in the schema is a compile error rather than a
silently ignored setting.
