# A GraphQL operation is a unit of work, and each mutation root field a savepoint

Settled on 2026-10-09 by the owner (one transaction per operation, a savepoint
per mutation root field, queries on the pool), landed on 2026-10-17. Before
it, a GraphQL request rode its HTTP request's unit: a batch holding a
mutation ran in `DbContext`'s transaction, which commits on any `2xx` — and a
GraphQL response is always `200`, whatever its `errors` hold. A mutation field
that failed after a write committed that write and dispatched its events,
while the docs promised a failed mutation never half-persists; a unique
violation poisoned the request's transaction and turned the typed GraphQL
answer into an empty `500`, where REST answers `409`.

**Chosen.** The transport marks a request to a mount whose request carries
units of its own (`CarriedUnits`: `/graphql`, `/mcp`), and the request's own
unit runs on the pool, its guards reading there. Each operation of the
document is a unit of its own, `graphql.operation`: a query on the pool, a
mutation in one lazy transaction. async-graphql resolves a mutation's root
fields one after the other and goes on past one that errs, so the `Mutation`
root runs each in an `atomic` block of the operation's transaction: rolled back
when the field's answer carries an error — its own, or one a field below it
reported, read off the response's error count, exact because no other field
resolves meanwhile — released otherwise. Every operation that executed then
commits: each failed field already rolled back alone, so a response never
reports work that was rolled back, and one whose commit cannot be honoured
answers `data: null` and the opaque `INTERNAL`. A batch holding a mutation runs
its operations in document order, so its units never hold several pooled
connections at once. Prior art: PostGraphile's savepoint per mutation field
inside one transaction per operation.

**Refused:**

- **All or nothing per request** — roll the whole batch back when any field
  errs. A GraphQL document is several independent fields by design, and a
  client reading one field's error cannot tell its sibling's successful
  answer was undone: the response would report work that was rolled back.
- **Tagging the HTTP response failed when any field errs** (`MappedError`, so
  `DbContext` rolls back). The same lie, one layer down: the `200` body still
  carries the other fields' data, and the transport cannot see which field
  failed.
- **One transaction per field.** A mutation's fields would each commit before
  the next ran, so the operation's later failure could not roll an earlier
  field back, and a document of N fields would take N round trips of
  `BEGIN`/`COMMIT` where one transaction and lazy savepoints cost nothing for
  a field that never touches the database.
- **Stepping the GraphQL endpoint onto the pool inside `DbContext`'s
  transaction** (what a query-only batch did). The guards' reads would open
  the request's transaction, and every mutation operation would then hold a
  second pooled connection beside it — a deadlock under pool exhaustion.

**The limits.** A driver offering no atomic block fails every GraphQL mutation
closed (`INTERNAL`) rather than commit a failed field's writes. A relation a
mutation's response resolves through a DataLoader reads the pool, so it does
not see the rows the operation wrote: running a mutation's batches on its own
transaction, joined before the field's block settles, is planned for 7.x
(`ambient-carry.md`) — it changes no API.

**2026-10-22 — a field that errs answers `null`.** An answer now follows
GraphQL §6.4.4 (`fix/graphql-errored-field-answers-null`): a root field whose
block rolled back answers `null` beside its error, where async-graphql left
its key out, and a Non-Null field's `null` climbs to its nearest nullable
parent. The blocks settle as before. Two consequences are left to the
owner, since each reopens how a block settles:

- A Non-Null mutation root field that errs (`Result<T>`, the usual shape)
  nulls `data`, while the root fields that answered keep their released blocks
  and commit: the answer reports none of the work that stands, and a client
  that retries the document applies it twice. Rolling the operation back
  whenever its `data` is nulled, and leaving the later root fields unresolved,
  as the specification allows, would keep the answer and the database in step.
- A root field whose block rolls back for a nullable field's error below it
  still answers its value, that field `null`: the response reports work that
  was rolled back. Settling the block on whether the field itself answers
  `null` — the specification's "the field errs" — would keep the promise.

**2026-10-22 — a root field settles on its own answer, and an answer with no
data rolls back whole.** Both consequences the entry above left open are
closed (`fix/graphql-mutation-settles-on-its-answer`):

- **A root field's block settles on what the field answers**, its errors'
  nulls placed — §6.4.4's "the field errs", the owner's 2026-10-09 "rolled
  back when that field errors" — no longer on whether its answer carries an
  error. A `null` that reaches the root field, its own or a Non-Null
  sub-field's, rolls the block back; one that stays below it, a nullable
  sub-field's or a Non-Null one's held by a nullable parent, releases it, and
  the field's value and work stand. Decided by the run from the owner's own
  words: the count-based rule rolled back a block whose answer stood. The block
  runs the placement the operation's answer gets, on the field's value as it
  resolved, so the two cannot disagree; an error async-graphql 7.2.1 files
  without a `path` (an interface field's) places no null, so its root field's
  work stands with its answer.
- **The owner's decision of 2026-10-22**, amending "released otherwise" for
  one case: when a mutation operation's `data` is nulled by a field error — a
  Non-Null root field's `null` — the whole operation rolls back and no root
  field after it resolves (§6.4.4 lets the siblings be cancelled); the skipped
  field answers `null` unresolved and adds no error. The answer reports
  nothing, so nothing it ran stands, and a retry applies nothing twice. Every
  other case keeps its savepoint per root field. The operation's unit settles
  on its answer: it commits only when that answer reports data. With no data
  layer bound the later root fields are skipped all the same: the answer
  reports none of their work.
- **Refused: resolving the later root fields and rolling them back with the
  operation.** Their guards and handlers would run, and whatever they do
  outside the transaction — an outbound call, a message sent at once — would
  happen for an answer that reports none of it.
