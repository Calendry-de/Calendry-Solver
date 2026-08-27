# Unary RPCs only; the solver owns in-flight run state and the app polls

Three unary calls: `StartRun` returns a run id immediately and optimizes in the
background, `GetStatus` reports progress and the best objective so far, and
`CancelRun` stops a run. No streams are held open.

A held-open stream would tie a run's lifetime to a connection, so a proxy
timeout, a redeploy or a client reload would look like a cancellation. Polling
makes the run's lifetime independent of any connection, and the app is already
persisting progress into its own table on each poll.

`buf.yaml` in the contract repo enforces this mechanically with the `UNARY_RPC`
lint, so a `stream` RPC fails the schema's own CI.

## Consequences

Progress is best-effort and sampled, not pushed. The registry must therefore
retain a finished run's result long enough for a poller to collect it — see
ADR-0020.
