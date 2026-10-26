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
