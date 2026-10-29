# Every refusal carries its class, beside the field map

A client branches on a refusal's class, the `code` core names for it, so one
class of failure answers one code on every edge. Two gaps were left open as
the owner's to decide — a new public name each, and a wire break — and the
owner answered both on 2026-10-26.

## 2026-10-26 — a code for every status the framework answers

A `405`, `406`, `408`, `410`, `413` or `415` the framework answered was a
problem document with no `code`, so a client could branch on its `status`
alone, and the code set could not place a new member: its rule read "the
HTTP status's reason phrase for an HTTP class, gRPC's canonical status name
otherwise", which derives neither `UNAUTHENTICATED` nor `RATE_LIMITED`.

- **Decided**: a class one HTTP status answers by its own name is that
  status's registered reason phrase in upper snake case — the IANA HTTP
  Status Code Registry's, RFC 9110 §15.5 for every status here, so the code is
  the document's `title` upper-cased: `METHOD_NOT_ALLOWED`, `NOT_ACCEPTABLE`,
  `REQUEST_TIMEOUT`, `GONE`, `CONTENT_TOO_LARGE`, `UNSUPPORTED_MEDIA_TYPE`,
  and by the same rule `PRECONDITION_FAILED`, `RANGE_NOT_SATISFIABLE` and
  `UPGRADE_REQUIRED`, the other statuses the framework writes (static files'
  `412` and `416`, the WebSocket handshake's `426`). A class the phrases do
  not name keeps the standard it was read off: gRPC's canonical names for a
  class several statuses share or whose phrase misnames it
  (`INVALID_ARGUMENT`, `UNAUTHENTICATED`, `UNAVAILABLE`, `INTERNAL`), RFC 6750
  §3.1 for `INSUFFICIENT_SCOPE`, RFC 6585 §4's *rate limiting* for
  `RATE_LIMITED`. The rule lives in core's `code` module doc, where the next
  member is placed, and a unit test beside `status_class` holds it.
- **Refused: renaming the nine** to fit one rule. A code is a public name a
  client already branches on; the rule says what each is read off instead.
- **Refused: a code for a status nothing in the framework writes** (`402`,
  `411`, `414`, `417`, `421`, `428`, `451`). No feature for a hypothetical
  user; the rule places each the day the framework comes to write one.
  hyper's `431` is written before the edge, with no document to carry one.

## 2026-10-26 — the class beside the field map, in band

HTTP and GraphQL named a pipe's refusal `INVALID_ARGUMENT` beside its field
map (`code` and `errors`, `extensions.code` and `extensions.errors`). A
WebSocket frame and an MCP error put the field map where the class rode —
WebSocket's `data.errors`, MCP's `data` — so a pipe's refusal there carried
no class, since a field may be named `code`. On WebSocket, `errors` meant two
things: the class members of a problem or a denial, or a pipe's field map.

- **Decided**: `errors` names the field map and nothing else, on every edge,
  and the class is its sibling. On WebSocket every error frame carries its
  class members in `data`, beside `error`: `{ error, code, reason?,
  requiredScopes?, retryAfterSeconds?, errors? }`, `WsError` typed member by
  member, the field map its one `serde_json::Value`. On MCP, `data` stays the
  class bag it is for a problem or a denial, and a pipe's field map moves under
  `errors` in it: `{ code, errors? }`. A frame or error a handler builds by
  hand (`WsError::new`, a GraphQL `Error::new`, an `McpError`) carries no
  class, as before.
- **Refused: the class in two places on one edge** — a denial's under
  `errors`, a pipe's in `data`. A client could not branch on one path.
- **Refused: the field map under a key of its own inside `errors`**
  (`errors.errors`, a reading proposed before). One word would name two
  things, and any other key renames the map every edge carries as `errors`.
- **The cost**, a wire break of WebSocket's error frame and MCP's pipe
  `data`, is one 7.0 takes; the upgrading guide states it.

## 2026-10-26 — a body that is no JSON-RPC message, on MCP

rmcp 3.5 answers a body typed `application/json` that does not decode as a
JSON-RPC message `415 Unsupported Media Type`, the answer of a wrong
`Content-Type`; with a code on every `415`, it would have read
`UNSUPPORTED_MEDIA_TYPE`, a wrong class.

- **Decided**, as MCP's Streamable HTTP transport and JSON-RPC 2.0 §5.1 say:
  input the server cannot accept is answered `400`, with a JSON-RPC error
  carrying no `id`, since none could be read (MCP's schema has no `null` id) —
  `-32700` Parse error for a body that is not JSON, `-32600` Invalid Request
  for JSON that is no message — in the code's words, under
  `INVALID_ARGUMENT`. rmcp's own sentence quotes serde's error, which may
  quote the body, so it is read by kind: rmcp reads every message through one
  untagged enum, so serde words any JSON of another shape in one sentence,
  and a unit test holds that reading against serde's own category.

## 2026-10-27 — an undeclared tool, at every MCP protocol version

An MCP address no host serves names `NOT_FOUND` in band, as HTTP's `404`
does; for a tool, that held at `2026-07-28` alone. The endpoint answers a
`tools/call` naming no declared tool before rmcp only where rmcp would look
the tool's schema up, from its `STANDARD_HEADERS` version, `2026-07-28`;
below it — `2025-06-18` and `2025-11-25`, today's clients — and on any path
holding a host whose router the mount cannot read, the call reaches rmcp's
handler, whose `ToolRouter` answers `-32602` `tool not found` with no `data`,
and the propagation's `said_by_kind` had no tool arm.

- **Decided**: the propagation classes it, where every handler's answer
  passes at every version — a lone host's router, the shared path's merge, a
  host written by hand, an endpoint built without an index. rmcp's
  `-32602` in its own words, with no `data`, on a `tools/call`, becomes the
  endpoint's own refusal: its code and message kept, `NOT_FOUND` added, the
  name in no reply. One crate-private reading of rmcp's sentence serves it and
  the merge's *not found*.
- **Refused: widening the endpoint's gate to every version.** It would read
  and parse the body of every `POST` below `2026-07-28` for a refusal the
  handler already answers; the gate exists for rmcp's unbounded schema cache,
  which only `2026-07-28` reaches.
- **Refused: classing in the composite.** An endpoint built by hand serves a
  host no composite wraps, and the propagation already holds every sibling's
  arm.
- **A refusal a host wrote with `data` of its own keeps it**, as does a
  resource's `-32002`: neither rmcp nor the framework writes one — rmcp's
  default `resources/read` is the `-32601` the class already reaches — so a
  `-32002` is a host's own answer, as it built it.
- **Left, as on `2026-10-26`**: rmcp's dispatch refuses `subscriptions/listen`
  and a tasks method a server advertises no tasks for, inside a session
  negotiated before `2026-07-28`, on the session's `200` stream before any
  handler runs; `StreamableHttpService` takes a `ServerHandler`, so no layer
  sits above that dispatch, and reading every frame of every stream is a cost
  on every answer.

## 2026-10-27 — rmcp 3.8: the endpoint's gate leaves, and its words move

rmcp's floor moved to 3.8 (`public-dependencies.md`, this date), and three
facts the entries above read off 3.5 moved with it.

- **The gate in front of rmcp's schema cache is gone.** rmcp 3.6 looks a
  tool's schema up per request and keeps nothing
  (`StreamableHttpService::tool_schema`, upstream #1349), so nothing grows on
  a name a client chose.
  A `tools/call` naming no declared tool now reaches rmcp's handler at every
  protocol version, `2026-07-28` included, which refuses it as the entry
  above classes it: `-32602` `tool not found` under `NOT_FOUND`, one
  `mcp.operation` line. The gate's own limits go with it: an id-less call is a
  notification rmcp accepts `202`, and no copy of rmcp's `Host` check is kept.
- **JSON of no message's shape is said in rmcp's words**, `unrecognized
  JSON-RPC message`, where 3.5 carried serde's untagged-enum sentence. rmcp
  3.8 also refuses a message carrying a stray `result` or `error`, or a
  repeated `id`, as no message (upstream #1283): each is `-32600` Invalid
  Request, as the 2026-10-26 entry decided for that shape.
- **A session-less request that is no `initialize` is `400`**, `Bad Request:
  Session ID is required`, where 3.5 answered `422` with a sentence of its own;
  a JSON-RPC response or error sent statelessly is `400` with `-32600`,
  `stateless server does not accept JSON-RPC responses`, kept as a sentence
  that quotes nothing, where 3.5 answered `202`.

## 2026-10-28 — a resource no `#[resource]` reads, written by the framework

The entry of 2026-10-26 left a `-32002` as a host's own answer, since neither
rmcp nor the framework wrote one. `#[tools]` now reads resources
(`mcp-resources-declared.md`), and the `read_resource` it writes answers a URI
no template reads, a variable that does not parse and a method's `None` with
MCP's own words, *Resource not found*, under the `NOT_FOUND` class and quoting
no URI — the one constructor, `nest_rs_mcp`'s `resource_not_found`, as
`tool_not_found` is for a tool. A `-32002` a hand-written host builds keeps its
`data`, as before.
