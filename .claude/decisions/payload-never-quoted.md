# A payload value is described, never quoted

`CLAUDE.md`'s hard "no" forbids a payload value or a secret in an error, a log
line, a stored record or a reply; a decode failure says where, what kind of
value and what the type expected. The reason is that every value the framework
decodes is somebody's data: a client's body, a producer's job, a deployment's
config. Quoting it moves that data into a log pipeline, a dead-letter record or
a `400` body, and a `400` is kept by proxies and caches the framework never
sees. A description carries everything a developer needs to fix the call.

## 2026-10-18 — a library's events are under the same rule: clamped, and opened by a directive

The framework's subscribers write what every library in the process emits, and
some quote a client's value or a secret: rmcp 3.5 at every level (a client's
`initialize` params, notifications and cancel reasons at `info`, its `Host` and
protocol-version headers and its JSON-RPC ids at `warn`, the session id it named
at `error` when the session store fails, each tool call's arguments and reply at
`debug` and `trace`), h2 0.4 a PING's opaque data at `warn`, rustls 0.23 a
client's SNI at `warn`, tungstenite 0.27 a close frame's reason at `debug`,
sqlx-postgres 0.9 a malformed `.pgpass` line with its password at `warn`, and
opentelemetry-otlp 0.33 its request URL — whose userinfo or query may carry a
collector's credential — and the collector's error body at `debug`. A line the
framework's subscriber writes is the framework quoting the value, whoever
emitted it, so the rule above reads on it.

Every subscriber the framework ships therefore narrows the filter it is given
by each such library's clamp: the library's events are written from the level
below which it quotes nothing at most (`rmcp=off`, `h2=error`, `rustls=error`,
`sqlx_postgres=error`, `tungstenite=info`, `opentelemetry-otlp=info`), and only
those the filter writes. The clamps are a second filter every event must also
pass, the clamps then the filter's own directives, so a directive naming the
library, or one of its modules, replaces its clamp (`EnvFilter` keeps the later
of two directives for one target): the opening is that visible line, and the
deployment's to write. The framework's own targets and the app's are never
clamped — a clamp reaching `nest_rs` does not compile — and what an app logs
stays its own: the framework adds no runtime restriction there.

- **A clamp never opens what the filter closes.** Prepended to the filter as
  directives, `tungstenite=info` outranks a bare `warn`, `error` or `off`, since
  a directive naming a target beats a level, and raises the process's maximum
  level — `tracing`'s and `log`'s — to `info` whatever the deployment set.
  Composed as a second filter, the stricter of the two decides, and the
  process's level stays the filter's.

- **The crate that wires a library declares its clamp**, beside the code that
  wires it, through link-time `inventory`, and the kernel collects them. A list
  in `nest-rs-core` would name libraries the kernel does not depend on — the
  central table `observability.md` refuses for targets — and the console
  subscriber starts before any module is known, which a link-time registry does
  not need. A clamp mounts and exposes nothing, so discovery's module gate
  (`forward-principal.md`) has nothing to gate: a linked library emits whether
  or not its module is imported.
- **The level is evidence, never usefulness.** rmcp at `warn` would keep its
  `Host` rejection line, and that line quotes the `Host`; h2 at `warn` would keep
  its rapid-reset limits, and an unsolicited PING ack's line quotes the PING. A
  clamp sits below the lowest level the library quotes at, and the framework's
  own lines say what the clamp hides that matters, described: the MCP mount
  files a refused `Host` at `warn` naming the variable that admits one, its
  sessions each failure they hand rmcp to answer, and its store each failure of
  the app's `SessionStore`, by kind and never the session a client named; and
  `nest_rs::http`'s `connection ended on an error` carries h2's reason. What
  stays the library's alone is said so where the clamp is documented — for
  rmcp, why a session's handshake failed in its worker, and an event store's
  persist failures, which the mount never reaches.
- **Refused:** patching or forking a library (an upstream issue is the owner's);
  filtering by message or field, which `EnvFilter` cannot do and each release
  would break; a clamp per module (`rustls::server`), which a refactor escapes
  silently and which the natural `rustls=warn` would not open.
- **Kept, as `EnvFilter`'s:** a directive matches every target its name begins
  with, so `rustls=error` also holds rustls-platform-verifier's root-loading
  warnings (`rustls_platform_verifier=warn` opens them alone), and a span
  directive (`[name]=debug`) opens what runs inside the span it names, a clamped
  library's events included.
- **A clamp is the audit of one version**: upgrading a clamped library, or one
  the audit cleared, re-reads its events.

## 2026-10-18 — the framework's own request lines: a refusal by kind, the server's values, the span's stated exception

The rule above reads on every value a client chose, success included, and on
every line and reply the framework writes. Its own request lines quoted three:
the browser-origin refusal named the `Origin` sent and the origin the server
took for its own, read off the `Host` or a trusted proxy's forwarded host; the
access line carried the `User-Agent` and an extension method as the client
spelled it.

- **A refusal line says the refused value by kind.** The browser-origin
  refusal files `reason` — the first check that failed, in RFC 6454's words
  (`scheme_differs`, `host_missing`, `origin_not_serialized`, …) — the sent
  origin's `scheme` from a closed set, whether the peer is a `trusted_proxy`,
  and a `hint` naming the settings that would admit it, as the MCP `Host`
  refusal does. What an operator changes is a setting, and the line names it;
  the value tells them nothing the kind does not.
- **The request's line carries the server's values only**: the route
  template, the method as OpenTelemetry's HTTP conventions know it — RFC
  9110's, `PATCH` and `QUERY`, spelled exactly, and any a route registers —
  or `_OTHER`, the status, the bytes and the duration. `user_agent` left it:
  free text, which a log pipeline keeps whole.
- **Every edge's line names what a client addressed by the server's own
  name for it**, and `_OTHER` — the conventions' word for a method they do not
  know — where the server declares none: a WebSocket message's line, its span
  and its `warn`s name the event a handler declares, and an event no handler
  serves is `_OTHER`, its error frame saying `unknown event`. The frame still
  answers on the event the message named: a protocol's correlation key echoed
  to its sender, on its own socket, is the protocol, as a JSON-RPC id is.
  An MCP operation's line names the protocol's method, `_OTHER` for a custom
  one, and a tool by the name a host declares (rmcp's `get_tool`), `_OTHER`
  for another. A value with no declared counterpart stays off the line: a
  GraphQL operation's name is the document's, so its line names the
  operation's type and its root-field lines what ran; an MCP prompt name,
  resource URI or task id, which no lookup answers for at dispatch.
- **A library's reply the edge passes on names nothing only the client
  named, as its line does.** rmcp refuses a prompt its host does not declare
  as `prompt '<name>' not found`, a custom method no host serves with the
  method as its message, and a request its transport turns away by quoting
  the `MCP-Protocol-Version`, `Mcp-Method` or `Mcp-Name` it carried;
  async-graphql refuses an `operationName` its document holds no operation of
  by quoting it, the request's free text rather than a GraphQL name. The edge
  says each by kind: `prompt not found`, the hosts' prompts still listed; a
  JSON-RPC error in its code's words, JSON-RPC 2.0's or MCP's, its id and
  code kept; any other refusal as its status's problem document; `Unknown
  operation named by operationName`. The library's wording is matched as the
  locked version writes it, and each edge's test sends a marker, so an upgrade
  that rewords it fails a test rather than quoting again.
- **The span keeps what the telemetry conventions require or recommend, as a
  stated exception.** The request span records `url.path` (required),
  `user_agent.original` (recommended) and, for a method outside the known
  set, `http.request.method_original` (required whenever it differs from
  `http.request.method`); a GraphQL operation's span its
  `graphql.operation.name` (recommended); an MCP operation's span its
  method, custom or not (required), and the tool, prompt or resource it named
  (each conditionally required, folded as `mcp.operation.name`), but no task
  id, which no convention names. Standards over invention: a tracing
  backend reads those names, and the span goes to the deployment's own
  collector, never to a client or a shared log. Beyond that set, a header the
  span records is recorded only once it parsed under its grammar — a
  `traceparent` the transport did not continue as the four hex fields it
  parsed to, an `X-Request-Id`, which no specification gives a grammar, only
  in the shape a proxy mints it, bounded hex (a UUID, or 16 to 64 hex digits)
  — and otherwise not at all: a token admits any word a client writes through
  a proxy that forwards the header. The span still records no `url.query` and
  no `client.address`, which the conventions also ask for: those stay out on
  the rule above and on personal data.
- **The route table is the override of the known methods.** The conventions
  ask an instrumentation that could turn a valid method into `_OTHER` to let
  the list be overridden, and name the variable an override by environment
  takes. A method a route registers outside the set — a mount answering
  `PROPFIND` — is the server's own word, so the line and the span name it as
  registered; no variable is read, the declaration being in code already.
- **Refused:** a hash or a prefix of a value to keep it greppable — a short
  value's hash is a dictionary away from the value, and a prefix is the value's
  start; a switch putting the values back on the line, whose place is already
  the span in the deployment's own collector.

## 2026-10-18 — a library's reply by kind, the document's names and MCP's version echo as stated exceptions, the span's `url.scheme`

async-graphql's validation and input coercion quote what a variable held: an
enum's value (`does not contain the value "…"`), an input object's key it does
not declare (`unknown field "…"`), the value a scalar refused (`Expected input
type "T", found …`), and a scalar's own message (`Failed to parse "T": …`),
which may quote it too. These are values from the request's `variables` JSON,
a key included, or written in the document — free text, not names the
GraphQL grammar bounds.

- **A value is said by kind, wherever the request carried it**: `does not
  contain the value sent`, `an unknown field of type "T"`, `found a string`
  (the kind read off how async-graphql renders it), and a scalar's message as
  `DecodeError` reports any type's own — serde's sentence rebuilt without its
  value, any other by its category. Matched on async-graphql 7.2's wording,
  each shape by a unit test and the edge's test a marker, so a release that
  rewords one fails a test rather than quoting again.
- **The document's own names are a stated exception, in the reply alone.** A
  field, an argument, a fragment, a variable or an operation name parsed under
  the GraphQL Name grammar (`/[_A-Za-z][_0-9A-Za-z]*/`) may stay in an error
  answered to the client that sent the document — the specification's
  validation rules name them, and every GraphQL server's errors carry them —
  and never on a line: the operation line names the operation's type, and no
  framework line or span carries async-graphql's message (its `tracing`
  feature is off). An `operationName` is the request's free text, not a parsed
  Name, and is said by kind.
- **A task id is said by kind in a refusal, as a prompt's name is.** rmcp
  3.5's `TaskManager`, the helper it offers a host writing `tasks/*`, refuses
  a task it does not hold as `unknown task: <id>`; the edge answers `unknown
  task`, matched on that wording against the id the request carried. The id
  reaches neither the span nor the line: no convention names it.
- **A library's refusal quoting nothing the request carried keeps its
  words**, matched exactly against the locked version's: rmcp's constant
  transport sentences (a missing `Accept`, a missing or unknown session, a
  body past the cap, a malformed `Host` or `Origin`) as the problem's
  `detail`, and its JSON-RPC refusals listing only constant names — the
  `_meta` keys the specification fixes, the headers it requires — as the
  error's message. Anything else stays by kind: an allowlist, never a
  denylist, since a reworded sentence must fall back to the kind rather
  than pass a value.
- **MCP's version-negotiation echo is a stated exception**, like a JSON-RPC
  id. An unsupported protocol version is answered with `data: {supported,
  requested}`, the version the client asked for echoed back to it: the
  2025-11-25 lifecycle's example error, and the 2026-07-28 draft's
  `UnsupportedProtocolVersionError`, whose `data.supported` and
  `data.requested` are both required, at HTTP 400. The shape is the
  specification's, the echo goes to the sender alone, and no framework line
  carries it.
- **The request span records `url.scheme`**, which OpenTelemetry's HTTP
  server conventions (Stable) require beside `http.request.method` and
  `url.path`: the scheme of the client's own request when a trusted proxy
  forwarded it — `Forwarded`'s `proto`, else `X-Forwarded-Proto`, read as the
  browser-origin check reads them — and it is `http` or `https`, otherwise
  the scheme the request arrived over. A closed pair of words, no value a
  client chose; the span exception above holds as written.
- **The `X-Request-Id` shape check has a residual, stated**: it records an id
  in the shape a proxy mints, so behind a proxy forwarding the client's
  header, hex the client wrote — a secret minted as hex or as a UUID
  included — is recorded as a minted id would be. The shape is all a header
  without a grammar offers to check.

## 2026-10-26 — a decode failure says where, in the names its type declares

SEC-03 and B-122 took the value out of every decode failure the framework
says; nothing said where. A query parameter a type refused answered `a value
its type does not accept`, naming no parameter; a JSON body named a line and
a column, never the field; a WebSocket payload, an MCP tool's arguments, a
job's payload and a structured config value the same.

- **Where is read off the type, never the payload.** A failed decode runs
  once more over the same payload under a tracker (`DecodeError::located`,
  nest-rs-core's `decode_path.rs`), only after it failed. A step is a field
  by the name out of the `fields` list a derive hands `deserialize_struct`,
  a variant out of `variants`, an element by its index; **any other key — a
  map's, or one a struct does not declare — is an entry, `[…]`**, its text
  never said: it is the sender's, as an unknown field's key is above. A
  field missing or given twice is placed at the field; a type's refusal in
  its own words keeps the fallback and adds what the visitor that refused
  expected, the type's words as serde's `expected …` tails are — nothing
  when the type refused a value a nested read had accepted (a `try_from`
  newtype, inside an `Option` or not), since what its holder expected was
  met.
- **What it costs.** A payload decoded from its text — a query, a form, a
  JSON body, a GraphQL request body, a config value, a WebSocket envelope —
  pays nothing on success: the text is still there to read again. A payload
  the edge already holds as a `serde_json::Value` — a WebSocket message's
  `data`, an MCP call's arguments, a job's payload — must outlive its first
  decode, so it is decoded by reference instead of moved: every string it
  holds is copied on every decode, about 90 ns on an eight-string payload
  (520 ns against 435 ns for its parse and decode, release build). Taken:
  correctness ranks above that copy, and a report says where on every edge
  or on none.
- **Each edge says it in the vocabulary its set already uses**: `query
  parameter `p``, `form field `p``, `JSON body at `p``, as `path parameter
  `id`` and `header `X-Api-Key`` did; the WebSocket envelope and payload
  frames (`invalid envelope at `event`: missing`), MCP's refusal (its
  `failed to deserialize parameters:` prefix whole, which rmcp's router
  reads) and a job's dead letter put the path in their own sentence.
- **A deployment's value is said by its place, never its text**: a boot
  refusal names the setting and where in it — a structured value's path, a
  URL's parameter by its position — as the `.env` cascade's refusals of
  `<PREFIX>_ENV` and of the prefix now do.
- **Refused: serde_path_to_error**, which the plan named. Its deserialize
  side records every object key as the payload's own text
  (`Segment::Map { key: String }`), a struct's field and a map's key alike,
  and a `deny_unknown_fields` refusal with the unknown key in its path: its
  rendering quotes what the client chose, and nothing it hands back tells
  the two apart.
- **What serde reads from a buffer of its own is placed at its holder**: an
  internally tagged or untagged enum's content, a `#[serde(flatten)]`
  field; a struct with a flattened field is read as a map, so its own fields
  are said as entries. Less said, nothing quoted.
- **Not located**: a multipart GraphQL request's `operations` and `map`
  parts, whose payload is gone (async-graphql returns its own error, not the
  part); a queue record that is no JSON at all, whose syntax names a line
  and a column; and a graphql-ws frame that is no protocol message, which
  stays `not a graphql-ws message`: async-graphql's `ClientMessage` is an
  internally tagged enum, read through serde's buffer, so a re-run would
  place nothing, and the answer is the engine's Close frame, whose reason
  RFC 6455 §5.5 caps at 123 bytes — the report, kind and expectation, is on
  the `debug` line. GraphQL's arguments and variables are named by
  async-graphql itself.

## 2026-10-27 — rmcp 3.8 quotes as 3.5 did

The 2026-10-18 entries read rmcp 3.5; 3.8, the floor since this date, still
writes a client's `initialize` params at `info`, the session id a failed
store load names at `error` and each tool call's arguments at `debug`, so
`rmcp=off` stays. Its events read against 3.5.1 add none on the server's
paths. The schema-cache gate, whose `Host` check mirrored rmcp's, left with
the cache (`every-refusal-carries-its-class.md`, this date).

## 2026-11-06 — an error is held to what it prints, by every reader

`error_message` and `SafeLink` are one reader. A `{:?}`, an `anyhow` report and a
`main` returning the error are others, and they print each link's own `Display` or
`Debug`, past every `SafeLink`. So a driver's error kept as a source is judged by what
it prints with a marked value in hand, never by what was read of it:
`nest_rs_testing::printed` is every reader at once, and a test holds the failure to it.

Run on 2026-11-06, a URL's password marked: seaorm's boot refusals quoted it once (a
scheme sea-orm has no driver for, which sqlx lets through, in sea-orm's own sentence),
and redis's never, on any topology, though its client repeated a `protocol=` value.
Each is closed at the input, so the refusals that hold keep their driver's error as
their source, `redis::RedisError` a public field among them. That holds for the answers
a server gives, which hold no password: a double answering `28P01` with the password in
its message has it printed. A storage
key held in object_store's errors is the opposite case: every such error quotes it, so
none is kept (`storage-port-by-url.md`, this date).

**A span is a reader too.** Every boot opened sea-orm's `connect` span, whose field is
the whole `ConnectOptions`, the URL's password included, at `trace`. The console prints
no span's fields and `LogCapture` reads events, so no suite saw it; under `RUST_LOG=trace`
an OTLP exporter ships it as an attribute. `sea_orm` is clamped at `debug` (read against
sea-orm 2.0.4: every span it opens is at `trace`, and its one event, at `warn`, names a
schema). **The limit:** a directive naming `sea_orm`, and an app's own subscriber, which
no clamp reaches, still read the span. Closing that takes a `ConnectOptions` whose URL
holds no password, the password set through `map_sqlx_postgres_opts`.

**Refused:** a sentence for every boot refusal of seaorm and redis, on the suspicion
alone. It takes the code and the server's words from an operator, moves a public field,
and fixes nothing a run showed. A library's retry lines were read the same way: with a
marked key, no line of object_store, reqwest or hyper_util quotes it at any level on a
refused connection or a failing status, so object_store takes no clamp (read against
object_store 0.14.2, reqwest 0.13.5).

**`ServiceError` and `ConfigError` are this family, and closed the same day.** A `{:?}`
of `ServiceError::Validation` wrote `params: {"value": String("…")}`, and an `anyhow`
report of it `password: Validation error: length [{"value": String("…")}]`, the rejected
value both times; a `{:?}` of `ServiceError::Db` wrote the server's `detail`, `Key
(email)=(…) already exists.`, and its report the server's message; a `{:?}` of
`ConfigError::Validation` wrote the rule's parameters. `error_message` said each without
them, so no framework line or reply quoted one: an app's own `?err`, `.unwrap()` or `main`
did. Each variant keeps its field, which `match` and `field_errors` read, and
`ServiceError`'s `Debug` and `source()` hand out a `#[repr(transparent)]` view of it whose
`Display` and `Debug` are what `error_message` says; a `ConfigError` prints its sentence.

**Refused:** no `source()` on those two variants, with a `SafeLink` claiming them.
`#[error(transparent)]` hands out the inner error's `source()`, skipping the error, so a
wrapper, the demo's `PostError::Service` among them, said `validation failed` alone in a
run, and an operator lost the cause. A view of the field costs `ref-cast`, already in the
tree through schemars, and asks nothing of a developer.

**The limit, each seen the same day.** `OpaqueCause`, beneath `ServiceError::Internal` and
`AuthError`'s failures, hands every reader but `error_message` the cause as it was given,
and `AtomicError::Database` the `DbErr` it boxes, the server's message in both its `Display`
and its `Debug`; a `DbErr` an app holds from `Repo` prints the same, being sea-orm's type.
None is a framework line or a reply. `QueueError::Serialize` keeps serde's error, and no
marked value was reproduced there. Closing the first two means holding `error_message`'s
sentence where the type-erased error sits, which moves what `source()` hands a reader of
every port error: a change of `nest-rs-core`'s own, and not this entry's.

## 2026-11-06 — a document that does not parse is told where it stopped

**A document the parser refuses was handed back.** Run this date: `mutation {
login(password: "…" }` answered ` --> 1:46`, a line `1 | mutation { login(password: "…" }`,
a caret and `= expected name`, pest's rendering of a syntax error, as the reply's
`message`, on `POST`, in a batch, in a multipart part and over the socket. A one-line
document of 600,014 bytes answered 601,025. No line held it, so the entry of 2026-10-18
holds for a line; a reply is what this rule forbids, and the document's names being the
sender's own never covered its literals.

The message now says the place and the grammar's words, `Syntax error at line 1 column
46: expected name`, from the request extension's `prepare_request`, the first reader
that may answer, over the failure the request's own parse left: the place is its
`locations`, and the message is read by an allowlist. The parser's sentences over what
the document declares stay, a name in them held to the Name grammar; a rendering's last
line is said only when pest's words make all of it, rule names after `expected` or
`unexpected`, or one of its limit sentences; anything else says the place alone. A
document that parses pays nothing.

**Refused:** matching the rendering's opening in `redact`, the shape every other
sentence there has. async-graphql is pinned, `=7.2`, and its sentences with it, but pest
floats inside `2.x` in an app's lockfile, where no test of ours runs: a rendering worded
differently would pass whole. What a later pest words otherwise says the place alone; a
last line of bare rule-shaped words would still pass, which is all the check holds.

**Refused:** a second parse to read the failure as `parser::Error`. pest bounds nesting
by the stack left, and two parses sit at different depths, so a document can fail the
request's parse on that bound and pass the second, whose answer then left the rendering
whole: seen this date on a real `POST`, debug build, 433 levels of `{a` around a literal
answering ` --> 1:895` and the line holding it. The place then also came from the second
parse, beside `locations` from the first.
