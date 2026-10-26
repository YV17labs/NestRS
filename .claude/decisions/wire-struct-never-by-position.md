# A wire type the framework writes is read from an object, never by position

serde's derive reads a struct from a map or from a sequence, its fields taken
in declaration order. A JSON array is therefore a second wire shape for every
derived struct, decoded by field order — undocumented, absent from every
schema the framework publishes, and silent: `["message", {…}]` is served as
the message `{"event":"message","data":{…}}`.

## 2026-10-26 — the WebSocket envelope, and the family it belongs to

`WsEnvelope` derived `Deserialize`, so a text frame holding a JSON array was
read by position: `["<word>"]` was answered as an unknown event named
`<word>`, and `["message", {…}]` served as a `message`. Its refusal of a
string also named the Rust type to the client (``expected struct
WsEnvelope``).

- **Decided**: a wire type the framework writes its own `Deserialize` for
  reads an object alone, its visitor's expectation said in the wire's words
  (``invalid envelope: invalid type: a sequence, expected an object with
  `event` and `data` ``), never a type's name; the fields keep serde's
  located errors (``invalid envelope at `event`: missing``). Three do
  today, each a type the framework decodes from a client's JSON:
  `WsEnvelope`, and the MCP endpoint's `Addressing` and `CallParams`, the
  `tools/call` whose tool it looks up before rmcp does, read through a
  map-only wrapper (`JsonObject`). Derived, these two read
  `[7, "tools/call", {"name": "x"}]`, and a `params` of `["x"]`, as a call
  to `x` on id 7, which the endpoint answered where rmcp's own reading, the
  one it mirrors, reads no call; an array now falls through to that
  reading.
- **The family, read**: `PageParams` is read from a query string and
  `AccessTokenRequest` from a form body (RFC 6749 §4.4.2), whose decoder
  (`serde_urlencoded`) has no sequence at its top: a positional read cannot
  reach them, so they keep the derive. `PageCursor` has no `Deserialize`: it
  is parsed from its text (`FromStr`). `Create<E>` and `Update<E>`, which
  `#[expose(input(...))]` generates, keep serde's derive, as a developer's
  own DTO does: they stand where a hand-written body type would, decoded by
  the same `Json<T>`, and refusing a sequence would take a hand-written
  visitor per generated struct — two per exposed entity, in the developer's
  crate, re-deriving what serde's attributes already say — for a shape whose
  values still pass the struct's validation and its posture. The GraphQL
  edge reads them as input objects, which have no positional form.
- **Upstream, read and not patched**: async-graphql's `Request` derives
  `Deserialize`, so a body `["{ x }"]` is read as the single request
  `{"query":"{ x }"}` before `BatchRequest`'s batch is tried; its
  `ClientMessage`, every graphql-ws frame the subscription socket reads, is
  tagged by `type`, and serde's tagged visitor takes an array's first
  element for the tag, so `["subscribe", "1", {"query": "…"}]` is read as a
  `subscribe` and `["complete", "1"]` as a `complete` (its refusal stays the
  constant `not a graphql-ws message`); rmcp's `JsonRpcResponse` and
  `JsonRpcError` (what a client answers to a request the server sent)
  derive it too, while its requests and notifications flatten their method
  and read an object alone. Each is the library's to change.
- **Refused: refusing a sequence in `Json<T>`**: the extractor cannot tell a
  struct from a `Vec`, and a body that is a list is a legitimate type.
- **Refused: a `#[serde(flatten)]` marker field**, which forces serde onto
  the map path: it buffers every field through serde's private content
  representation on every decode, and adds a field a struct literal must
  name.
