# The documentation is a development surface, opened in production by one visible line

Settled on 2026-10-08, under the owner's mandate to decide the OpenAPI audit's
open questions by the project's values. OWASP API9:2023 asks that "API
documentation [be] available only to those authorized to use the API", and
ASVS 5.0 13.4.5 that documentation endpoints are "not exposed unless explicitly
intended".

`OpenApiModule` serves `/api` and `/api-json` by default in the development and
test profiles only. Outside them a deployment opens them with
`<PREFIX>_OPENAPI__ENABLED=true`, and the boot logs that the endpoints are public
at `warn`. Security decided: safe by default, and the opening is one visible
line.

**Refused:**

- **The documentation behind the app's own guard chain.** The API's guards read
  a bearer token, which a browser navigating to `/api` cannot send, so a
  guarded page could never load the UI that asks for the token. A deployment
  that publishes its documentation to a restricted audience gates it where
  browsers authenticate: its proxy or its SSO.
- **Filtering the document per caller** (OpenAPI 3.2 §6.4 allows it): a
  document that differs by caller cannot be committed, diffed or generated
  from, which is what it is for.

Reopened when the framework gains a browser session a guard can read.

**2026-10-28 — GraphQL's playground and introspection are the same surface.**
Introspection answers the schema, and the playground is a page reading it:
both are API documentation in API9's sense. `GraphqlConfig` kept them off in
every profile, so each app opened them by hand — the demo turned the
playground on and left introspection off, a page with no schema to read.
`GraphqlConfig::defaults()` now serves the playground and answers
introspection in the development and test profiles, as the documentation and
Server-Timing are served; `Default` keeps both closed, and a deployment
opening either (`<PREFIX>_GRAPHQL__PLAYGROUND=true`,
`<PREFIX>_GRAPHQL__INTROSPECTION=true`) is said once at `warn`. Each key takes
the shape `server-timing-exposure.md` gives a development surface — on in
development and test, off in `Default`, saying what it serves — so
`DISABLE_INTROSPECTION` became `INTROSPECTION`, its opposite; one `ENABLED`
would name the endpoint itself, which is the API and always served. A
subgraph's `_service` still publishes its SDL whatever introspection says, and
stays behind a router. The two committed copies — this
document and the SDL — share one key pair, `EMIT` and `EMIT_PATH`, written by a
development run alone and resolved against the directory of the crate
`#[nest_rs::main]` expanded in, so each app of a workspace writes its own.
