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

`NESTRS_HTTP__ACCESS_LOG` stays: it predates the family and is an app's pinned
config, not a deployment's filter. Four more booleans for the other edges would
be four declarations of a decision `nest_rs::operation=off` already makes.
