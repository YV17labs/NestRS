# A request's default deadline sits above every wait under it and inside the shutdown window

Settled on 2026-10-11, when the HTTP edge's default request timeout moved from
30 to 20 seconds so it would fit the shutdown window (20 s): with the old
default, a request admitted just before a stop ran inside its own budget and was
cut unanswered by the window. Correctness decided: a default the framework ships
holds its own guarantees.

The order every default keeps, from the inside out:

1. **Budgets** — what one call to a backend may take: Redis's command budget
   (10 s), the Postgres statement bound and the storage operation (15 s each), a
   social login's three provider calls (5 s each).
2. **Nets** — what a port waits before it calls its backend unable to answer:
   the authentication guard's `AUTHENTICATE_TIMEOUT`, the throttler's
   `HIT_TIMEOUT`, the queue port's `BACKEND_TIMEOUT` — **18 seconds**, above
   every budget under them, so an outage is reported with its own cause.
3. **The request deadline** — 20 seconds, counted from the request's arrival.
   What a request waits on under it gives up strictly before it: the deadline
   starts first, so at equal values it always answers in that wait's place,
   with a `503` naming nothing.
   - A net, or the hung backend it is built to name is never heard on HTTP.
   - The body-read gap — 10 seconds, half the deadline — or a body that stops
     arriving is told `503` with a `Retry-After`, an invitation to retry, where
     RFC 9110 §15.5.9 names `408`. Half rather than just under: the gap counts
     from a wait on the body, so a stall is answered `408` only when it starts
     before the deadline less the gap — in the request's first half.
4. **The shutdown window** — 20 seconds, the request deadline at most, then the
   hooks and the flush inside Kubernetes' 30-second grace period. A deployment
   whose request timeout outlasts its window, or has none, is told at boot with
   one `warn`; it is not refused, since a long-running route may accept the cut
   knowingly.

**Refused:**

- **Nets left at 20 seconds, equal to the request deadline.** Read above: equal
  is not below when one clock starts first.
- **A wider window (21 s, the request deadline 21 s, the nets 20 s).** It keeps
  every value but one, and leaves half a second between the way down and the
  grace period, where a pod is killed without a word.
- **Lower budgets under 15-second nets.** It moves four crates' defaults —
  Redis, Postgres, storage, the OAuth client — to keep three constants round.
- **Nets at twice the Redis budget, as a margin for a deployment that raised
  it.** The boot already refuses a budget at or past each net it reaches;
  the room left (8 s over the 10 s default) is what the order allows.
- **The body-read gap at the head deadline's 30 seconds.** Past the request
  deadline it never fired: at the defaults every stalled body was answered the
  deadline's `503`.
- **Refusing a request timeout past the window at boot.** A deployment serving
  long uploads with the budget off is a legitimate one; the fact supports a
  `warn`, not a refusal (`container.md`, *Discovery*).
