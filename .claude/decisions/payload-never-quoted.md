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
  9110's, `PATCH` and `QUERY`, spelled exactly — or `_OTHER`, the status, the
  bytes and the duration. `user_agent` left it: free text, which a log
  pipeline keeps whole.
- **Every edge's line names what a client addressed by the server's own
  name for it**, and `_OTHER` — the conventions' word for a method they do not
  know — where the server declares none: a WebSocket message's line, its span
  and its `warn`s name the event a handler declares, and an event no handler
  serves is `_OTHER`, its error frame saying `unknown event`. The frame still
  answers on the event the message named: a protocol's correlation key echoed
  to its sender, on its own socket, is the protocol, as a JSON-RPC id is.
- **The span keeps what the telemetry conventions require or recommend, as a
  stated exception.** The request span records `url.path` (required),
  `user_agent.original` (recommended) and, for a method outside the known
  set, `http.request.method_original` (required whenever it differs from
  `http.request.method`). Standards over invention: a tracing backend reads
  those names, and the span goes to the deployment's own collector, never to
  a client or a shared log. Beyond that set, a header the span records is
  recorded only once it parsed under its grammar — a `traceparent` the
  transport did not continue as the 55 hex characters it parsed to, an
  `X-Request-Id`, which no specification gives a grammar, only in the shape a
  proxy mints it, bounded hex (a UUID, or 16 to 64 hex digits) — and
  otherwise not at all: a token admits any word a client writes through a
  proxy that forwards the header. The span still records no `url.query` and
  no `client.address`, which the conventions also ask for: those stay out on
  the rule above and on personal data.
- **No override list for the known methods.** The conventions ask an
  instrumentation that could turn a valid method into `_OTHER` to let the list
  be overridden. The router serves no method outside the known set, so a method
  an app answers is never `_OTHER`; a verb the router learns joins the set in
  that change.
- **Refused:** a hash or a prefix of a value to keep it greppable — a short
  value's hash is a dictionary away from the value, and a prefix is the value's
  start; a switch putting the values back on the line, whose place is already
  the span in the deployment's own collector.
