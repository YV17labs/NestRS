# No CSRF layer while no framework credential rides a cookie

Decided on 2026-10-14, with the OAuth transaction cookie moving into the
framework. Cross-site request forgery needs a credential the browser attaches
on its own; a nestrs credential is a bearer token in `Authorization`, which a
browser never sends unasked, so a forged cross-site request arrives with no
credential and is refused as anonymous.

The one cookie the framework writes is the OAuth transaction
(`OAuthClient::TRANSACTION_COOKIE`). It is no credential to a state-changing
route: it is read on the provider's `GET` callback, and that leg is held by the
`state` comparison (RFC 6749 §10.12), PKCE (RFC 7636) and the transaction's
binding to its provider. Its attributes are the framework's for the same
reason: `__Host-` so no sibling subdomain plants it, `HttpOnly`, `Secure`,
`SameSite=Lax` so the provider's top-level redirect still carries it back.

So 7.0 ships no CSRF middleware. **Reopened the day a framework credential
rides a cookie** — a session module: origin-based CSRF ships with it, through
`Sec-Fetch-Site` and `Origin` (OWASP's Fetch Metadata defence), with no token
to thread through forms.

The `Origin` check on a WebSocket upgrade, a graphql-ws upgrade and `/mcp` is
not this layer. It is the transport control those protocols name — RFC 6455
§10.2 for an upgrade, which a browser opens without consulting CORS, and the
MCP transport's requirement that a server validate `Origin` on every incoming
connection — owed by their own edges, and it holds whatever the credential: a
strategy reading a cookie or a query parameter on an upgrade is protected by
it, not by a CSRF layer. It admits the request's own origin and the ones
`<PREFIX>_HTTP__CORS_ORIGINS` lists, one origin list for both, as Spring's and
Socket.IO's handshakes use theirs.

**Refused:**

- **Shipping `Sec-Fetch-Site` CSRF now**: it would guard a credential no
  framework code accepts — a feature for a hypothetical user.
- **A synchronizer token**: it needs server-side state or a second cookie,
  and Fetch Metadata answers the same threat from headers every current
  browser sends.
- **rmcp's `allowed_origins` for MCP alone**: it cannot say "the request's own
  origin", and a second mechanism for one concern breaks one way to do a
  thing: the three entry points share one `OriginPolicy`.
- **Checking `Origin` only when CORS is configured**: with CORS off, the
  default, every cross-site page could open a socket: the configured case was
  sound, the unconfigured one was the hole.
