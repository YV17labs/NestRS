# A wait the other side asks is honoured, and spread above it

## 2026-11-08 — never earlier than asked, at most a fifth later

Two loops of the framework meet a wait the other side asks: a job whose failure
carries a `Problem` with `with_retry_after` (an upstream's `429` passed on), and
the JWK Set fetcher meeting an issuer's `429` or `5xx` with a `Retry-After`. The
queue honoured the ask exactly, so every job one `429` refused at one instant came
back at that instant plus what it asked, and met the same limit together. The JWK
Set fetcher did not honour it at all: it passed the issuer's wait on to its own
callers in their `503`, then asked the issuer again at its 30-second floor (seen
red: an issuer asking 120 seconds was fetched again at 30).

Both now wait what was asked, counted up to a ceiling of an hour (`ASKED_CEILING`
for a job, `JWKS_RETRY_AFTER_CEILING` for the fetcher), stretched by a factor in
`[1, 1.2]`: `nest_rs_core::Backoff::honouring`, the upper half of the jitter every
retry loop of the framework already takes, drawn by the same FNV-1a from a seed and
the attempt — the job's id for a job, `process_seed()` for a replica's fetcher — so
it is reproducible and sets apart what one answer told to wait. RFC 9110 §10.2.3
says how long the client ought to wait; a client waiting longer is within it, one
coming back sooner is not. The `503` the fetcher answers while it waits now says the
time until its next fetch, not the issuer's whole wait.

**Refused:**

- **The backoff's own `±20 %` around the ask.** Its lower half comes back before
  the upstream asked, which the specification does not allow and which spends an
  attempt on a refusal known in advance.
- **A random draw.** Every wait of the framework is a function of its seed, so a
  test pins it and an operator reading two lines can tell why they differ.
- **Jittering past the ceiling, or holding the jittered wait to it.** The ceiling
  bounds what an upstream may ask; held after the stretch, every job asked an hour
  or more would come back at the same instant again. The longest wait is the
  ceiling and a fifth: 72 minutes.
- **A stretch wider than a fifth.** It is the jitter's own width, which the
  framework's reconnect loops already spread over; a wider one delays a job an
  upstream would already take.
