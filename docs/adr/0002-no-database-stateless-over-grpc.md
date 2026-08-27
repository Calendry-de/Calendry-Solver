# The solver never touches Postgres

Nuxt assembles a `SolverInput` snapshot and sends it over gRPC. This service is
stateless and input/output-only: no database, no DB driver, no on-disk run
journal, and no persistence beyond an in-flight run's lifetime.

The alternative — letting the solver read the same Postgres the app owns — would
duplicate the app's row-level security, its multi-tenancy rules and its schema
knowledge in a second language, and would make the solver's correctness depend
on migrations landing in two repositories at once. A snapshot has one owner.

## Consequences

Every fact the solver needs must be expressible on the wire. When something is
missing, the schema changes (ADR-0003) rather than the solver reaching for it.
Run state dies with the process, which is why the app persists progress into its
own `solver_run` table.
