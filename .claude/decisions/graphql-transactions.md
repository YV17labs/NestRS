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
  runs the placement the operation's answer gets, by the same document — the
  one the request sent, so a copy a directive leaves out counts in both — on
  the field's value as it resolved; with each root field resolved once (the
  next bullet), the two cannot disagree. An error async-graphql 7.2.1 files
  without a `path` (an interface field's) places no null, so its root field's
  work stands with its answer.
- **A mutation names each root field once.** async-graphql 7.2.1 runs a root
  field once per selection, where §6.3.2 merges one response key into one
  execution, and merges the two answers into one: a repeated mutation field
  wrote twice, and `mutation { a: shaped { broken } a: shaped { fine } }`
  answered `{"a": null}` while one copy's block was released. A mutation whose
  root selections — through its fragments, one fragment spread twice, a copy a
  directive skips included — name one response key more than once is refused
  before any root field runs: `data: null` and one `INVALID_ARGUMENT` error at
  the repeated selection, its unit opened and settled with nothing in it.
  Decided by the run on correctness: it is the one answer that neither writes
  twice nor reports work that was rolled back. A query naming a key twice
  still runs as async-graphql runs it, since it writes nothing.
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
- **Refused: running a repeated root field once, its selections merged, as
  §6.3.2 does.** async-graphql resolves each selection through its own call
  and gives no hook between validation and execution, so the merge would be a
  rewrite of the document before validation, which would then judge a
  document the client did not send; and a later copy cannot answer nothing,
  since its key lands in the answer whenever the first copy erred.
- **The limit, of the placement both read:** a copy a directive leaves out
  still counts as selected, so an object selected twice below a root field,
  one copy skipped and the other's every field failed, answers `null` (or its
  nearest nullable parent does) where the specification answers the object
  with its nulls — and the root field's block reads the same, so the two
  still agree. Counting only the copies that run needs the request's
  variables beside the kept document.

**2026-11-05 — the mark's name.** No `CarriedUnits` exists: `/graphql` and `/mcp` say their request carries units with `HttpEndpointMeta::carries::<V>()`, and the request runs on the pool as above.

**2026-11-05 — a document is refused once validation passed, and read by what runs.** Two lines above stand corrected, by a run of each (the `Test first` of the commit). "Gives no hook between validation and execution" is true of a merge, which needs the validated document, and false of a refusal: `Extension::validation` wraps async-graphql's `check_rules`, with the request's data already attached, so a repeated root selection now rides in that data and the schema's one extension answers it after validation passed. `mutation { a a unknownField }` says the unknown field. The extension is the one the schema had, moved to `crates/nest-rs-graphql/src/request_extension.rs` and named for what it does, since a second would cost a boxed call per field. The limit is closed: the operation's read and the kept document hold the request's variables, the kept document only when its text holds an `@`, and both drop a selection by async-graphql's own rule (private there, so copied and pinned by a test): `{ leaf { broken } leaf @skip(if: true) { fine } }` answers `{"leaf":{"broken":null}}`, and `mutation($x: Boolean!) { a @skip(if: $x) a @include(if: $x) }` runs `a` once. The rule reads `request.variables` alone where an argument reads the definition too, so the operation's read fills an absent variable's default into them, after the global pipes, which see what the request sent. A Non-Null variable absent with no default is §6.1.2's request error, refused the same way: async-graphql ran the operation, reading `@skip(if: $x)` as `false` and so running its write. Decided on correctness. **Refused: refusing the required variable only where a directive reads it.** §6.1.2 draws no such line, and an argument reading one left the root fields before it committed. **The federation ceiling is the third reader of `request.variables`** (`refuse_oversized_entity_calls`): a list carried as a variable's default counted as none and walked past `max_representations`, and counts now because the operation's read runs first, held by `a_variables_default_counts_against_the_representations_ceiling`.

**An object with no key.** async-graphql 7.2.1 answers `null` for one where §6.3 answers `{}`, and `insert_value` drops whatever finishes after a `null`, so a selection its directives leave nothing to answer hides the other selection's answer whenever it finishes first, by the order they are written or by which resolver returned first, with no error. A document holding an `@` is read for such a selection, once per fragment, and only one holding one pays anything more: beside a selection that answers, the operation is refused once validation passed, which is what bounds the walk of every merged position (`max_complexity` counts through fragments); at a Non-Null position, which has no `None`, the object answers `{}`, and the operation's root as before. Decided on correctness: a silent answer that depends on a race is worse than a refusal naming the fix. **Refused: answering it by a rewrite** (a key added to the empty set, stripped from the answer), for the reason the merge above is refused: validation would judge a document the client did not send. **The limit that stands:** at a nullable position that `null` stands, since a resolver's `None` reads the same, so `{ org { name @skip(if: true) } }` answers `null`; and a type condition leaves an object keyless by what the value turns out to be, which no read of the document knows, so there the `null` stands at any position and can hide another selection's answer. Reading every answer that selects through a type condition is a walk on the hot path for a case `__typename` closes in the document. Canaries fail on a release that answers `{}`.

**A read of a document before async-graphql's `check_recursive_depth` stops at its depth** (`RECURSIVE_DEPTH`, 32, the default no module raises), since that check refuses whatever lies below. Corrected by a run: the mutation root's read followed a fragment chain on the stack, and a chain of 5,000 fragments, under 200 KB of body, overflowed it in a test build and aborted the process. `the_read_of_a_mutations_root_reaches_as_deep_as_a_document_runs` fails on a bump that moves the limit.

**2026-11-06 — a document's cost is counted before anything walks it.** Seen, each by a run, on a release build and with no credential: `query { ...F0 }`, each fragment spreading the next twice, held a worker 10.3 s at depth 26 for 1,068 bytes; ending in a `__typename`, which async-graphql 7.2.1's score skips, it was admitted and resolved too; a chain of 20,000 fragments no operation spreads, about 700 KB, overflowed the stack and aborted the process, since its rules recurse through what `check_recursive_depth` never reads; thousands of unused fragments spreading one, 1.37 MB, cost 33.9 s, their square; `[$v, …]`, ten thousand references to 100 KB in a 130 KB body, took peak memory from 12 to 1,550 MiB, the argument rule building the list from clones, which extrapolates to hundreds of GB within the default body cap; each reference scans the operation's definitions, inside one poll of the resolver, so 55,000 definitions and 205,000 references, 1.97 MB, held a worker 7.98 s and were answered; and `OverlappingFieldsCanBeMerged` looks each argument of a repeated response key up in the other's, so a key repeated with 115,000 arguments, 2.08 MB, cost 9.39 s (these last two on a build without LTO). The walks are all async-graphql's: `RootSelections` and `Blanks` read each fragment once, so nothing is to be fixed there but `is_skipped`, which copied a variable to read a directive's `if`, ahead of the count, and now does not. `crates/nest-rs-graphql/src/document_cost.rs` stands in `Extension::prepare_request`, the one hook ahead of them, since `check_recursive_depth` runs inside the parse future. Each decided on Security, then Correctness.

- **What is counted is what a document adds to its text**, never its total: a fragment spread again adds what it expands to, and a document spreading each one once adds nothing, whatever it scores, so a field scored below one, whose document costs what it weighs, stays the visitor's to judge. For a document of default scores the count refuses only what the score would, since what repeats add is less than the fields it scores, a `__typename` apart, which the score skips and the count does not: its walk costs what any field's does.
- **No new key.** `max_complexity` means the fields a document may expand to, so it holds the repeats; off, it scored nothing the deployment had to bound, and the repeats stop at the most it may say, ten million, past which `MAX_COMPLEXITY` already calls one document a denial of service. What arguments copy is held to twice what the request may carry, read off the request: `max_body_bytes` on a `POST`, `max_message_bytes` on a graphql-ws socket, so a raised cap raises it and the default 64 KiB frame holds a socket to 128 KiB.
- **A value counts no more than its text weighs, so every variable may be referred to twice.** Counting what it takes in memory, as a first version did at 8 bytes a scalar, refused one reference to 600,000 small numbers inside the body cap, a request the transport had admitted. What that costs: a million of them, a 2 MB body, referred to twice, peak at 233 MiB, 160 MiB of it before any copy, where half as many, the most that first count admitted, peak at 128 MiB. It stays linear in the body, and lowers with it.
- **Copies are read off what async-graphql does, proved by runs:** 10,000 references to 100 KB cost 29 MiB in an operation `operationName` leaves out, or when it names no operation, and 1,126 MiB with no name among several operations, or in a fragment only a left-out operation spreads, a fragment's arguments being copied whichever runs. So the count holds the larger of what the operations run and what the fragments' own arguments copy once.
- **A step of a scan counts as a byte**, on the same limit, needing no new key. At each reference, the definitions down to its own, where `Context::var_value` stops, read once per variable with its size. At each field, its arguments squared, in every operation since validation reads them all: telling a repeated key apart would cost every selection set a table, and counting one that is not repeated costs nothing real, since the default limit first refuses about 2,000 arguments on one field. **Refused: refusing a field holding more arguments than any the schema declares**, which needs the registry's read, and answers a typo on the schema's widest field with `Query is too complex.` rather than the unknown argument it is. At the limit the reads ahead cost 4.4 ms (a key repeated with 1,400 arguments) and 8.5 ms (2,001 definitions, 3,001 references).
- **A fragment no operation reaches is refused unread**, the first in the text, in `NoUnusedFragments`' own words. Counting it instead was refused: read leaf first, a memo never sees how deep its chain goes. The cost is that such a document, invalid whatever it holds, reads one fragment rather than the list, and none of its other validation errors.
- **The refusals are async-graphql's sentences**, `Query is too complex.` with no location and the recursion bound's at its selection set, since a client has no reason to tell the two checks apart and one fact is one sentence. Neither carries a code, as neither did.
- **Refused: a time budget.** The walks are synchronous, so nothing could stop one; the deadline answered 29 s into a 20 s bound. **Refused: lowering `limit_recursive_depth`.** Width multiplies as depth does: three levels of 64 spreads, 1,081 bytes, were 262,144 expansions. **Refused: a key of its own**, which no deployment could set from anything it knows.
- **The limit that stands.** A document repeating its fragments, whose fields the schema scores under one each, can be refused for its repeats where its score alone would pass. Closing it means scoring here, against the registry and each field's `compute_complexity`, which is the walk this exists to avoid. The remedy is to raise `max_complexity`, which holds both. Nothing in the demo or the docs comes near it.
- **The other limit that stands: what a list's rows copy.** A field under a list reads its arguments at each row, through `Context::var_value`'s scan and `.cloned()`: `query($v: String) { rows { f(x: $v) } }`, 1,000 rows, a 1 MiB variable, a 1 MiB body, scoring 2, peaked at 1,037 MiB. The registry could tell a reference stands under a list, not how many rows that list answers, so charging a guess would refuse small values or admit large ones. Closing it takes async-graphql sharing a variable's value rather than cloning it. **Refused: a budget per request charged as each field runs.** It reads the arguments of every field holding one on the hot path, and it counts a sum where the harm is a peak, so a page of 100,000 rows each copying 64 bytes would be cut into a partial answer. So a request with no credential can still reach this on any public list field taking an argument: a security gap, left open and named to the owner. Until upstream moves, the remedy is on the field: `complexity` scoring the list by its rows and a rule on the argument, `#[validate(length(max = 1024))]` bringing that request to 45 MiB, since each row's refused copy is dropped before the next.
- **Measured**, release: the count costs 30 ns on a three-field document with no fragment and no variable, where it cost 3 ns reading nothing before a field's arguments were counted, 333 ns on a hundred fields, 75 ns with one variable, 233 ns with three fragments and 1.56 µs with thirty. A whole request runs as it does with the count off within noise: 21.5 to 22.1 µs against 21.8 to 22.4 µs without a fragment, 34.6 to 35.5 µs against 34.4 to 34.8 µs with three. A document that does not parse pays one more parse, its fourth.

**2026-11-07 — a required variable sent `null` is refused as one left out.** The 2026-11-05 entry left it read only where the position reading it refuses it, since nothing ran that the client did not ask for. Seen false: `query($hint: String!) { hinted(hint: $hint) }` sent `null` answered `{"hinted": false}`, a nullable argument handed the `null` §6.1.2 forbids, and a mutation reading one in its second root field ran its first. The operation's read now refuses it before anything runs, at its top or at any list item its type declares Non-Null, a lone value standing where a list is declared read as its one item (§3.11). Only the variable's own wrappers are read: what its named type holds is validation's, which reads the value against the position's type, and `VariablesInAllowedPosition` makes that the same named type, so no registry is read. Decided on correctness.

**2026-11-07 — what a list's rows copy is held to a bound of its own.** The 2026-11-06 entry left it open, "the other limit that stands", for want of the row count. Seen again on this tree, release, no credential: 1,000 rows of a field reading a 1 MiB variable, a 1 MiB body, peaked at 1,026 MiB. The row count stays unknowable, so what is bounded is what one row copies: the argument values of the fields a list stands above, their custom directives' and their fragments', a reference counted at what it costs. That is what multiplies, and what no other limit reads: a 1 MiB body may copy 2 MiB in all, and a row's copy is never more than that. Decided on Security, then Performance:

- **A key of its own, `max_list_argument_bytes`**, 4 KiB by default, from 1 to a gibibyte, `None` or `0` off, overlaid and pinned as its siblings. No existing key means it: `max_complexity` counts fields, `max_body_bytes` what a request carries, and neither says what one row may copy. 4 KiB holds a cursor, a page size and a filter at each of several connections, and the demo's and the docs' documents sit far below it; a thousand rows then copy 4 MB where they copied a gigabyte.
- **The schema is read only past the bound.** The count already reads every argument; what stands below any field is summed beside the rest, which bounds what stands below a list, so a document within the bound by that sum pays one addition per argument and no lookup. Past it, the operation the request runs is walked once more against the schema's types, each fragment at most twice, by whether a list stands above its spread, a field the schema does not declare left to validation.
- **What is read once is not counted**: a root field's arguments, and a field only objects stand above, so a mutation's large input passes. `@skip` and `@include` are read before anything runs, so their arguments and what they leave out copy nothing at a row.
- **The refusal is `Query is too complex.`**, as for the other counts.
- **A document's values are read in place**, through `async-graphql-value`'s `Value`, the parser's type, which async-graphql re-exports only in its const form: the count cloned every argument to weigh it, and a directive's `if` was cloned to read it, where a match by reference costs nothing.
- **Refused: bounding the row count.** It is the resolver's, known only once the list ran. **Refused: a budget charged as each field runs**, for the reason the 2026-11-06 entry gives. **Refused: waiting on async-graphql** to share a variable's value rather than clone it at each row, which `TODO.md` lists to report; the bound stays useful after it, for literals.
- **The limit that stands**: a page of 100,000 rows each copying 4 KiB still holds 400 MB of copies, so the list's own size is the field's to bound, by its `complexity` and its page.

**2026-11-07 — every object answers an object: a set that could answer no key is given one.** The 2026-11-05 entry on an object with no key refused a rewrite and left three things standing: a nullable object a directive empties answering `null`, a type condition leaving an object keyless and hiding another copy's answer, and a refusal of a blank selection beside another that read by response key, so refused a document the specification admits. Each was seen again on this tree (the suites' red, `a_nullable_object_every_selection_of_which_is_left_out_answers_an_empty_object`, `an_object_no_type_condition_of_which_applies_answers_an_empty_object`, `no_copy_hides_another_selections_answer`), and so was a member none named: `{ org { broken } org { id } }`, a copy whose every field erred, answered `org: null` and lost `id`. The value is the resolver's, so no read of the answer can tell `{}` from `None`; the document can be made to answer, so it is. Each selection set that could answer no key is handed `__typename` under a key of its own (`crates/nest-rs-graphql/src/own_key.rs`), taken out of the answer before its nulls are placed: a set every selection of which a directive leaves out, one some possible type of its value meets no type condition of, and every copy of a response key selected more than once at one place, with what stands below it. The refusal goes, and so does the Non-Null patch, which the key makes `{}` by itself. Decided on correctness, then performance.

- **The refusal of the rewrite is lifted, its reason met.** It said validation would judge a document the client did not send. What is added is `__typename` alone, on a set the schema says is an object, an interface or a union, where it is always valid: it scores nothing in async-graphql 7.2.1's complexity, nests nothing past what the set already does, merges with every other copy of itself, and its key is drawn for each request (`__nestrs_key_` and sixteen hex digits, the process's secret hash keys over a count), so neither the document nor a value the answer holds spells it but by a 64-bit guess: the answer loses it from every object, a scalar's value among them, which no read of the document could tell apart where async-graphql admits one response key answering a scalar under one type condition and an object under another (it runs no `SameResponseShape`). So every rule says of the document what it said before, and a refusal names only what the client wrote, held by `a_document_validation_refuses_says_what_validation_says_of_it`. The merge stays refused, for its own reason: it moves what the client wrote.
- **Given after the count, read against the schema.** The sets are read once the document's cost was counted, which bounds what each fragment's spreads expand to, in `Extension::prepare_request`, and the copy carrying the keys replaces the cached parse. A type condition applies as async-graphql's `Fields::add_set` applies it: the concrete type, an interface it implements, or the type its position resolves. So a condition on a union applies only where that union is resolved, never to a member selected elsewhere, where §6.3.2 *DoesFragmentTypeApply* applies it: `{ shape { ... on Either { ... on Shape { fine } } } }` answers `{"shape": {}}`, the set given its key, where the specification answers `fine`. Left open, upstream's to close or a rewrite's (`TODO.md` §4 and §7), held by a canary that turns red on the bump fixing it.
- **The hot path.** A document holding no fragment, no directive and no object selected twice at one place reads no type: every set selects a field. Only a document given a key pays the copy and the walk of its answer. Measured, release, logging off, a request 2,000 times, three rounds, each tree rebuilt before its run and the two alternated twice, the median of each run, before and after this branch: no fragment 24.4 and 23.7 µs against 23.7 and 24.6; one fragment covering its type 23.5 and 22.8 against 22.7 and 23.0; a directive keeping a field 19.9 and 19.8 against 19.7 and 19.6; a directive emptying ten rows, which the Non-Null patch walked before, 20.0 and 19.9 against 17.9 and 17.7; an object selected twice at one place, now given its key and its answer walked, 27.8 and 26.8 against 28.7 and 28.9, about 1.5 µs, the one cost, paid by that document alone.
- **Refused: reading the answer for the `null`s.** A resolver's `None` reads the same, so the answer cannot tell them apart, and what a hiding `null` dropped is gone. **Refused: keeping the refusal for the copies.** It refused documents the specification runs, and a type condition's case it could not read at all.
- **The limit that stands**: an `@defer` or `@stream` the schema does not declare is refused as before, and a mutation root naming one field twice is refused as before, since each copy would run its writes.

**2026-11-08 — a list of lists selected twice with different selections is refused.** The 2026-11-07 entry said no copy hides another's answer once each could answer a key. Seen false below a list of lists: `{ grid { fine } grid { inner { fine } } }`, `grid: [[Shape!]!]!`, answered one copy's cells and lost the other's, with no error, and so did two fragments each selecting `grid`, and copies of `grid` below two copies of its parent. async-graphql 7.2.1's `insert_value` joins a later copy into the first only where both are objects, or lists whose items are objects; a list's items that are lists are never joined, whatever key each object holds, and its `OverlappingFieldsCanBeMerged` compares copies at one set alone. Decided on correctness, then the developer's own code, which nothing here touches:

- **Refused, as the mutation root's repeat is.** The pass that gives a set its key now reads the merged selections, a value of each possible type at a time, and refuses the operation once validation passed when a response key's copies answer a list nested in a list and select otherwise: `INVALID_ARGUMENT`, at the first copy that differs, naming the key. Copies alike — one field, argument list and directive list, `@skip` and `@include` aside, which were read before, and what runs of each selection set alike — lose nothing, and are answered, so no document answering right today is refused. Copies under conditions no one value meets never meet, and are answered.
- **Refused: merging the copies before validation.** A named fragment's copy would move into another's at every place it is spread, and inlining it leaves it unused for `NoUnusedFragments`: the rewrite the mutation root's line in `TODO.md` §4 already names, which closes both.
- **Refused: answering each copy under a drawn key and joining the answers back.** Validation would no longer compare the copies, and every resolver, extension and error path would read the drawn key.
- **The hot path.** Read only when the walk met a repeat; each place once per type it declares, a group of copies alike read through its first, so a document repeating one key 20,000 times reads it in a fraction of its parse.
- **The limit that stands**: copies selecting otherwise that would answer alike (`{ a b }` beside `{ b a }`) are refused too, the comparison reading selections rather than answers; the merge closes it with the rest.
