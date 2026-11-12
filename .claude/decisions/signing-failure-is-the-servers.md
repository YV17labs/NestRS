# A token this server cannot sign is the server's failure

## 2026-11-05 — `SignError`, and why it is no `AuthError`

Decided on the owner's mandate of 2026-11-05 ("Je veux que tu corriges
absolument tout ce que tu viens de mentionner"), which covers the public API
break. `JwtService::sign` answered `AuthError::Failed` on a verify-only service,
and `AuthError` is a `ResponseError` answering `401` with a challenge, so the
`self.jwt.sign(&claims)?` the JWT page taught told a client to fetch another
credential for a key this server does not hold. Seen run: `401`, on `sign`,
`sign_for`, `sign_handshake` and `OAuthClient::authorize` alike.

- **Taken: a `SignError` of its own**, returned by those four, by
  `SocialProvider::authorize` and by `SocialRegistry::authorize`. It displays a
  constant and keeps what failed as its source.
- **Taken: `ToProblem` saying `None`, not a hand-written `ResponseError`.** That
  seam is the one mapping every edge reads, so `?` answers the opaque `500` on
  HTTP and the opaque answer on a WebSocket, GraphQL or MCP handler that mints,
  and the edge files the chain at `error`, with no line of its own here. `.opaque()`
  is no longer needed, and still compiles.
- **Refused: `AuthError::Signing`, a `500` variant.** It would spare every caller
  the type change, and it would make the enum say something else: `AuthError` says
  why authentication established no identity. A strategy's `?` would reach it, on a
  path where `#[public]` absorbs a refused credential as anonymous, and `sign`'s
  signature would keep promising `Expired`.
- **Refused: `From<SignError> for AuthError`.** It could only fold into `Failed`, the
  `401` this removes. A login handler `?`-ing both a `CredentialError` and a
  `SignError` answers `nest_rs::http::Result`, into which each converts on its own
  status; a service holds `#[from] SignError` in its own error, and
  `TokenError::Sign` now does (it held an `anyhow::Error`, so `?` works at a token
  endpoint with no `map_err`). This is the one place the fix asks more of a developer:
  a service typed `Result<_, AuthError>` that signs no longer compiles. Neither the
  demo, the `g auth` template nor a docs page was written that way.
- **Not covered:** `OAuthClient::exchange` and `verify_handshake` on a service that
  cannot verify a handshake keep `AuthError::Failed`: they run on the callback, a
  guard's path, and an app reaching them began no flow, since `authorize` refused
  first.

## 2026-11-05 — the redirect's fields are private, so a cookie leaves with `no-store`

`AuthorizationRedirect` held `pub url` and `pub transaction`, with a public
`set_cookie()`. Its `302` carries `Cache-Control: no-store`, but a handler reading
the two could write the cookie on a response a cache keeps, and a provider's
override could append what no header can carry, which the answer then refused at the
last moment with an `OAuth redirect rejected` line. Seen run: `redirect.url.push('\n')`
compiled and reached that line.

- **Taken: `location` and `set_cookie` private, as `HeaderValue`s validated where
  `authorize` builds them.** `IntoResponse` is infallible and the refusal line is
  gone; `url()` reads where the browser is sent, and the cookie has no reader: the
  answer is its one exit.
- **Refused: a setter appending to the URL.** No provider in the framework appends,
  and a validating one is additive when a provider asks.
