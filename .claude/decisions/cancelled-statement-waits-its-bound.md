# A statement Postgres cancelled names its pool's statement bound as its wait

Settled on 2026-10-19, by correctness, from a finding of the 7.0 foundation
run's eighth checkpoint: with the table locked, the HTTP edge's own deadline
answered `503` with `Retry-After: 3`, while a `#[crud]` statement Postgres
cancelled at its `statement_timeout` (`57014`) answered `503` with neither a
code nor a wait.

The deadline's `Retry-After` is the budget that ran out, rounded up and never
`0` (`nest-rs-http`'s `deadline.rs`). A cancelled statement is the same
failure one layer down — the server could not serve the unit in time — and
the budget that ran out there is the statement bound of the pool the unit ran
on. `nest-rs-seaorm` reads it off the unit's ambient executor's pool, the
options its connections open with, and says it once, in the `Problem` every
edge renders: `Retry-After` on HTTP, `retryAfterSeconds` on GraphQL, WebSocket
and MCP, for `#[crud]` and `ServiceError` alike. Outside a unit, or on a pool
opened without a bound, no wait is invented.

**Refused:**

- **The request timeout, as the deadline's `503` states it.** The HTTP request
  timeout is the edge's, carried in the request's extensions: an error mapping
  reaches it only through the `#[crud]` expansion or a new public marker the
  site stamps, it exists on no other edge, and it names a budget that is not
  the one that ran out.
- **A wait stamped by the HTTP edge on every `503` that carries none.** It
  would invent one for a failure that gave none — an identity store's outage
  answers without a wait unless the store named one.

Reopened if a unit sets a statement bound of its own, below the pool's.
