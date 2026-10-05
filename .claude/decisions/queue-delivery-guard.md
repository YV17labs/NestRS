# The delivery guard filters the common duplicate and promises no more

Delivery is at least once. apalis delivers a job twice in ways no setting of its
public API removes (its startup sweep reclaims every registered consumer's
in-flight jobs, a replica missing its heartbeats is swept, an acknowledgement is
lost in a drain), so the Redis worker guards each delivery with a lease and a
settled mark kept for one fixed span.

The first cut kept the marks of jobs settled around a drain or a lost
acknowledgement for a week. That grew a set per method with the orphan
threshold, ran passes the stop waited out past its window, and still missed an
acknowledgement lost during a pass: machinery buying a promise the queue does not
make. It was removed by decision. A redelivery after the mark lapses runs the job
again, and the handler is idempotent.

Superseded (2026-10-05) by `queue-redis-streams.md`: on Streams every transition
is fenced on the pending entry, so the guard, its lease and its settled mark go.
