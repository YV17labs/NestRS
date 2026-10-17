# Server-Timing is a development surface, opened in production by one visible line

Settled on 2026-10-10, applying the principle of
`openapi-docs-exposure.md` to its sibling. The W3C Server Timing draft
(Working Draft of 7 April 2026, §4 *Privacy and Security*) warns that the
header can "expose potentially sensitive application and infrastructure
information", and lets a server "only provide certain metrics to correctly
authenticated users and nothing at all to all others".

`ServerTimingModule` used to stamp every response with the time each backend
took — the sub-steps a handler records name them — the refusals included, so a
caller an authentication path turned away could read how long it worked. It now
stamps by default in the development and test profiles only. Outside them a
deployment opens it with `<PREFIX>_SERVER_TIMING__ENABLED=true`, or a pinned
`ServerTimingConfig { enabled: true }`, and the boot says so at `warn`. Whatever
the profile, a `401`, `403`, `407` or `429` carries no timing: the refusal of a
credential, of a proxy's credential or of a rate is where a timing oracle
reads. The boot decides once, at register, from the config the factory phase
resolved: switched off, no wrap is attached, so the transport keeps its fused
path and no request pays for it. Security decided: safe by default, and the
opening is one visible line.

The refusal is read off the status line, and that is its limit: `/graphql`
refuses an operation with `200 OK` and an error frame, `/mcp` a tool or prompt
call with a JSON-RPC error, and both keep their timing wherever the header is
on — the guard chains answering them run inside the request the header wraps
and know nothing of it. Closing it takes an in-band refusal to mark the
request it answers inside: under `one-execution-pipeline.md` a nested unit runs
inside the HTTP unit whose unit stage holds the header, so its guard stage can
leave that mark. Until then the docs page states the limit, and the default,
off outside development, is the answer for a deployment serving those edges to
callers it does not trust.

A later development surface takes the same shape: an `ENABLED` key, off in its
config's `Default`, on in development and test through its profile's
`Config::defaults()`.

**Refused:**

- **A per-route opt-in.** The header is a whole-app diagnostic measured from
  the edge in; a switch per controller would make the deployment's posture a
  property of each route instead of one line an operator reads.
- **An allow-list of client origins or addresses.** Who may read a response is
  the proxy's decision, which sees the client. `Timing-Allow-Origin` only lets
  a browser's script read the metrics; the wire carries them to anyone, so an
  origin list in the app would hide nothing from a `curl`.
- **A check inside the interceptor on every request.** A switched-off header
  would still cost each request a lookup; the boot reads the config once.
- **Reading a `200`'s body for an error frame.** It would buffer every GraphQL
  and MCP answer, streams included, to decide one header.

Reopened when the stage that stamps the header can read the authenticated
caller, so a deployment could give its timings to its operators alone, or when
an in-band refusal can mark the HTTP request it answers inside, so `/graphql`
and `/mcp` refusals go out untimed too.
