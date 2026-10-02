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
