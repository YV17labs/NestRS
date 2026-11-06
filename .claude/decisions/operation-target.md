# Operation lines share one target, and it is not a subsystem word

Every edge's per-unit line is `nest_rs::operation`. Through 5.1 it was
`nest_rs::access`, HTTP's word for its access log surviving the generalisation
to six edges — a clock tick has no caller, so nothing accesses anything.

`nest_rs::access` also prefixed `nest_rs::access_graph`, and because
`EnvFilter` matches a directive with `starts_with` on the raw string, the
documented toggle silenced the boot warning naming unreachable GraphQL
resolvers. That is why no target may prefix an unrelated one; a family root
(`nest_rs::oauth` over its three role crates) is a level a reader sees in the
path, and is the intended exception.

Spans and lines used to be two vocabularies (`http.request` beside "request
served"); a unit is now one typed constant read by both.

`<PREFIX>_HTTP__ACCESS_LOG` stays: it predates the family and is an app's pinned
config, not a deployment's filter. Four more booleans for the other edges would
be four declarations of a decision `nest_rs::operation=off` already makes.

## 2026-11-06 — `method` is the Rust method, and a line names its unit as its `mounted` line does

The semver page freezes a line's field names at 7.0, and `method` held three
things on this one target: the HTTP verb on `http.request`, the JSON-RPC method on
`mcp.operation` and the Rust method everywhere else. `operation` held two. "An
edge's operation line speaks its protocol" described that code rather than a
reason: an operator's filter on `method` crossed three vocabularies, and the
line's protocol already rides the span by its convention's name.

So `method` is the handler's, `http.request` says `verb`+`path`, `mcp.operation`
says `mcp_method` beside `tool|prompt|resource`, and GraphQL says `kind`+`field`.

`http.request`, `ws.message` and `mcp.operation` carry no `host` or `method`: each is
filed by its edge around the dispatch, and joins its `mounted` line through its
coordinates. Adding the two fields is additive, a slot on each line, so it waits for a
caller.

Refused: `method` kept on `http.request` beside a `handler` field for the Rust one.
That renames the set's stable member to spare the one line that drifted.

Refused: `kind` holding `entity` and `federation`, as `role` did. On a denial and a
`mounted` line `kind` is GraphQL's operation type, so a filter on `kind="query"`
would miss one line of the set; the line's `host` and `method` say which it was, and
the span keeps `graphql.operation.role`.

A `resource` is what a `#[resource]`'s unit says it matched, carried back through an
ambient slot on `resources/read` alone, because the URI is the client's and the line
is filed outside that unit. A hand-written `ServerHandler`'s resource reads `_OTHER`:
nothing declares a template the edge can read at that moment.
