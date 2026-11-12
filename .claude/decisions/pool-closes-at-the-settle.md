# A pool's close waits for what its units lent out, and no longer

Settled on 2026-10-17, after one API shutdown in six spent the whole
five-second hooks budget on `DatabaseConnection`'s close. sqlx's
`Pool::close` takes every connection back before it says goodbye to any, so
whatever still held one held the close: a connection whose statement Postgres
still ran — a pool unit cut at its deadline under a table lock — until that
statement ended, and one whose release had failed for good. The shutdown
observed was held by the second: sqlx 0.9 pings a connection on its way back, the ping met the
statement-timeout error of a statement a cut unit had dropped, and the
`close_hard` that follows flushes a rustls socket by waiting for the server to
send — on a quiet connection, forever, its pool permit with it. Correctness
decided: by the time a pool closes, the binding's own close has drained every
unit's settle, so a connection still out belongs to no unit, and waiting for
it buys nothing but the budget.

**Chosen — take back what lands within the settle, then close.** The close
takes each idle connection as it lands for `SHUTDOWN_SETTLE_TIMEOUT` — a
settled unit's connection comes back after a round trip, and no event says
when — then marks the pool closed and hands them back, so sqlx says goodbye
to each on its own task. What is still checked out then closes as it comes
back, and one `warn` on `nest_rs::orm` counts it.

**Refused:**

- **sqlx's close under a shorter bound.** Its goodbye to the idle
  connections comes after every permit is back, so a bound that fires leaves
  them to fall with the process, the one thing the close exists to avoid.
- **Cancelling the statement a cut unit left running.** sqlx 0.9 keeps a
  connection's backend process id and secret key private, so a
  `CancelRequest` cannot be sent, and `pg_cancel_backend` would need a fresh
  connection at shutdown and every connection's pid learnt in advance.
- **Finishing sqlx's close on a task of its own.** It would outlive the app
  that opened the pool, until a statement nobody waits for ends.

The `close_hard` hang is sqlx's to fix; reporting it upstream is the owner's.

## 2026-10-31 — what is still checked out falls with the process, and the goodbyes are awaited

The entry above said a connection still checked out at the close closes as it
comes back. It does not, under sqlx 0.9: a cut unit's connection starts back
at the cut, before the close, on a task that pings it and so waits for the
statement Postgres still runs. A statement that then ends is released into
the closed pool's idle queue, which nothing pops once the close returned, so
the connection falls when the pool drops; one that fails — cancelled, or past
its `statement_timeout` — fails the ping, and sqlx closes it hard, the flush
above that never returns on a quiet TLS socket. Neither sends a goodbye. Seen
on the e2e test that cancels the statement after the close: the server kept
the connection idle until the test process exited.

**Chosen — say what holds, and await what can be.** The `warn` now says each
such connection falls with the process, and the docs say so. The connections
the close took back say their goodbye before it returns, awaited in place
rather than handed to sqlx's tasks, which the process's exit could cut.

**Refused — a goodbye for the connection still out.** It needs a drain of
the close's own on every release (an `after_release` callback pinging before
sqlx's ping, to read the pool's state once the statement ended): a round
trip more on every statement the pool serves, for a connection the process
is about to drop. sqlx closing gracefully what it releases into a closed
pool, or after an error the server answered, is upstream's, beside the
`close_hard` hang the owner reports.
