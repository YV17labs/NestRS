# A client's error is said by its unit's line, and the edge's report of it at `debug`

The project map keeps `warn` and above for denials and security. A client
that sends what an edge cannot serve — a payload that does not decode, a value
a pipe rejects, an address no handler declares — is neither: it is the
client's error, a client can make one per request or per frame, and a `warn`
it triggers at will is a flood an operator learns to filter, hiding the
denials that share the level.

## 2026-10-26 — the WebSocket edge's refusals of what a client sent

The WebSocket edge filed a `warn` on `nest_rs::ws` beside the `ws.message`
line for a payload that does not deserialize, a pipe's rejection and an
unknown event, and a frame it could not read as an envelope filed nothing at
all, neither line nor report. The other edges already said one client error
once: HTTP's `400` for a body, a query or a pipe, and its `404`, file the
access line alone; MCP's refusal of a tool's arguments and of a tool no host
declares, its operation line alone; GraphQL's refusal of a body that is no
GraphQL request, a `debug` line beside its request's.

- **Decided**: every refusal the edge makes of what a client sent, before a
  handler runs, files the unit's line `error` — under `_OTHER` where the
  client named nothing a handler declares — and reports what failed, and
  where, at `debug` on the edge's target, inside the unit's correlation. A
  frame that is no envelope, a binary frame and a frame past the in-band size
  cap are messages like any other: each files its `ws.message` line and span.
- **What an operator keeps**: the line, at `info`, filterable on
  `outcome="error"`; the where, at `debug`. Nothing moved to a level a
  deployment cannot open.
- **Refused: moving the decode failure alone**, the smallest change the
  defect asked for. A pipe's rejection is the same payload refused at the same
  edge, and an unknown event the same client error as HTTP's `404`; one of
  three at `warn` would still make the level mean nothing.
- **Not decided here**: a handler's own `Err` answered as a `4xx` on the
  WebSocket edge is still filed at `warn` with its chain (`Line::Answered`),
  where HTTP, GraphQL and MCP file their line alone. It is the handler's
  verdict rather than the edge's refusal, and the edges rule's error opacity
  decides what a handler's error files.

## 2026-10-26 — a handler's refusal on the WebSocket edge, decided

The entry above left a handler's own `Err` answered as a `4xx` to the edges
rule's error opacity. It filed a `warn` with its chain on `nest_rs::ws`
(`subscribe_message handler returned Err`) for a `WsError`, a `4xx`
`Problem`, a pipe's rejection or a decode failure the handler returned, while
the same failure answered `4xx` by an exception filter filed nothing, and
HTTP, GraphQL and MCP file their unit's line alone for it.

- **Decided**, as error opacity reads: a handler's error is filed where the
  unit's answer is final, by the status the client is sent, whoever answered
  it — the handler, by its error's type, or an exception filter. A refusal
  files the message's `ws.message` line alone, `outcome="error"`; the chain
  behind a `503` is filed at `warn`, the chain a `5xx` withholds at `error`,
  each worded as the other edges word theirs (`websocket message
  unavailable`, `websocket message failed`); a bare `Problem` withholds
  nothing and files nothing beside its line. One rendering serves the
  handler's error and a filter's answer, so the two cannot drift again.
- **No `debug` report of a handler's refusal**: HTTP files none for a
  handler's `4xx`, nor do GraphQL and MCP. The frame already says what the
  handler meant the client to read, and the line says which message failed.
- **Refused: the chain at `debug`**, the smallest move. It would make the
  WebSocket edge the one that says a handler's refusal twice, and the chain
  of a refusal is the handler's own words, which its frame carries.

## 2026-11-05 — and its span records nothing

The entries above took a client's error off `warn`. Its span stayed: a
`ws.message`, a `graphql.operation`, a `graphql.field` and a `graphql.subscription`
filed `error` recorded `error.type` and an `Error` status whoever earned it. So a
client could set `Error` on the server's span once per frame or per operation, and
a backend's error rate read its typos as the server failing, where HTTP's `4xx` and
MCP's caller codes fail nothing on theirs. The observability rule's "anything but
`ok` exports as `error.type`" had read as this edge's default, not as a decision
about the caller's own failure.

- **Decided, by correctness**: a failure the caller earned files its
  `outcome="error"` line and records nothing on the span, neither `error.type` nor
  a status. A `5xx`, a panic, a stop, a message or an operation cut at its deadline,
  and an operation its unit could not honour record as before. A graphql-ws socket
  the protocol refused is one such failure: the client broke it.
- **The class is read off the answer's status, not its code**: an app's own code on
  a `Problem` answering `507` is the server's. The frame carries it beside its wire
  members, serialized as nothing; a GraphQL error carries it as its `source`, which
  no response serializes. Each is set where a status is known: `Problem::status`,
  `Denial::http_status`, the edge's opaque answer.
- **A WebSocket filter's reply changes what the client reads, never who failed**, and
  names no status to read. So the view keeps the class of the failure the edge
  rendered, which `WsView::respond` leaves alone: a server failure a catch-all answers
  `WsReply::error` records, a client's `409` it acknowledges with a reply does not. A
  failure the edge never rendered, an interceptor's `Err` a filter answered, reads what
  the edge would have answered it. A `Caught::Problem` is rendered, so its status
  decides, as HTTP's filter's does. GraphQL's filter answers only a `Problem`, and
  HTTP reads its status, so neither holds this question.
- **Refused: reading the class off the frame the view holds at the end**, which a
  filter's reply decides in both directions, and **re-reading the failure once the
  answer is final**: a `ToProblem` error's chain names no vocabulary, so a domain
  `409` would read as opaque, and a `Caught::Problem` is forgotten by then.
- **What names no status is the caller's**: a `WsError` a handler built with `new`,
  an `async_graphql::Error` a body built, what validation refused. Error opacity
  already reads each as a refusal, its line alone, so the span follows it. A body
  answering a failure of the server's in those words records nothing; its line
  still says `error`, and the `5xx` problem is its spelling.
- **Refused**: naming the code on the span, which no convention for either edge asks,
  and `otel.status_message`, an added field carrying a developer's sentence to an
  exporter, which is the no-payload rule's own decision and additive.

## 2026-11-07 — the class the view keeps is the rendered failure's, by identity

The view kept the class of the failure the edge rendered, whatever failure reached
the end: an interceptor swapping that failure for another, which a filter then
answered through `WsView::respond`, left the first one's class on the span — a
client's `409` swapped for a disk error recorded nothing, a server failure swapped
for a `409` recorded `Error`. Seen by a test, both ways. The view now keeps a
handle on the failure it rendered (`nest_rs_core::__private::kept`, which shares
its error, so no later failure takes its address) and reads its class only for that
same failure (`is_same_failure`); a failure a layer raised in its place reads what
the edge would have answered it, as one the edge never rendered always did. One
handle per failed message, an `Arc` clone. **Refused: comparing the error's
address alone**, which a failure dropped by the swap could hand to the next one
allocated.
