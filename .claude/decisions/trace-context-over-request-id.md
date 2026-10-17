# W3C Trace Context replaced the home-made request_id

The framework's correlation primitive is W3C Trace Context. A homemade
`request_id` answered the question `trace_id` answers, so the two were
duplicates. The three defences offered for keeping it are retired by the
specification: a forgeable inbound id is answered by the spec's *restart trace*
mutation at a trust boundary; the standard needs 16 random bytes and a
`format!`, not an SDK; and a trace-id sorts by time if its bytes do — ours are a
UUID v7's, a conformant trace-id and an ordered UUID whose right-most 7 bytes
still satisfy the `random-trace-id` flag. OpenTelemetry's semantic conventions
have no request-id concept, only the captured header
`http.request.header.x-request-id`, so `X-Request-Id` is upstream data recorded
behind a trusted peer, never an identity.

**2026-10-17 — W3C Baggage beside it.** A job's envelope accepts the W3C
`baggage` key and propagates it, as the specification has a platform pass on
what it does not understand: kept for the attempt within W3C's limits (64
list-members, 8192 bytes), written into every push the attempt makes, dropped
with a `warn` that never quotes it otherwise. Nothing in 7.0 writes baggage
from a request or reads a member, and a value the framework's posture depends
on — the actor, the tenant — is a first-class key, never baggage, since anyone
writing to the queue can forge a member. Reserving the key in the 7.0 wire
format is what lets a later minor add members without a version bump.
