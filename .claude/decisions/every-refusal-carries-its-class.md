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
