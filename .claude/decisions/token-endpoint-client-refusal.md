# A refused client is answered by the method it attempted

Decided on 2026-10-21, when `TokenError::InvalidClient` stopped sending a
`401` with no challenge. RFC 9110 §15.5.2 requires every `401` to carry one,
and RFC 6749 §5.2 requires the token endpoint, when the client authenticated
through the `Authorization` header, to answer `401` with `WWW-Authenticate`
matching the scheme it used. The variant therefore carries the
`TokenEndpointAuthMethod` the request attempted, read off RFC 7591 §2's
`token_endpoint_auth_method` registry:

- **`ClientSecretBasic`** — HTTP Basic in the header, or any scheme there,
  since Basic is the one this server reads — is answered `401` with
  `WWW-Authenticate: Basic realm="authorization server"`.
- **`ClientSecretPost`** and **`None`** — credentials in the body, or none —
  are answered §5.2's own `400 invalid_client`, with no challenge.

The realm is a constant. RFC 7617 requires one; RFC 7235 §2.2 scopes it to
the server's canonical root URI, so one name per deployment is one protection
space; and it names what every endpoint taking these client credentials
shares — the token endpoint today, revocation (RFC 7009) and introspection
(RFC 7662) if they come — after RFC 6749 §1.1's role, which is also this
crate's subject.

The standard decided the `400` (§5.2 makes the `401` mandatory for a header
attempt only, and `400` is its status "unless specified otherwise"), then
security: a `Basic` challenge to a request that never used HTTP
authentication has a browser carrying a public client raise a credentials
prompt. Authlib answers the same way.

**Refused:**

- **`401` with the `Basic` challenge for every refusal**, which §5.2 allows
  ("MAY") to advertise the supported scheme: the prompt above, for no
  client that reads the header.
- **A realm the app configures**: no machine client reads it, and the crate
  has no configuration seam to hang it on — a setting for a hypothetical user.
- **`Bearer` on the challenge**, as some servers answer: it is the resource
  server's scheme, never the one a refused client used.

**Not covered:** a client authenticated by `AuthnGuard` at a token endpoint
(the demo's `/token`) is refused by the guard's `401` with a `Bearer`
challenge and a problem document, not §5.2's `invalid_client`. Reopened with a
client-authentication guard in `nest-rs-oauth-server`, or a strategy that
declares its challenge.

## 2026-10-26 — a client-authentication guard, and the token request's own refusals

The *Not covered* above is closed: a client a token endpoint authenticates is
refused by `ClientAuthenticationGuard<S>` in `nest-rs-oauth-server`, in §5.2's
words, through `TokenError`'s one rendering.

- **Taken: a guard over authn's `Strategy`**, refusing through `TokenError`,
  the HTTP guard stage serving the error a refusing guard writes into its view
  (`HttpView::respond`). One way to check a credential — the app's strategy,
  unchanged — one rendering of §5.2, and no public name in `nest-rs-guards` or
  `nest-rs-http`. The run — the bound, the net, the contained panic, the
  principal attached — is authn's, called through a hidden seam, never copied.
- **Refused: a strategy that declares its challenge**, the other road above. A
  strategy maps a request to a principal and answers nothing on the wire;
  declaring a challenge would have `AuthnGuard` render two protocols, and §5.2's
  `400 invalid_client` is no challenge at all — no `Denial` renders it.
- **Refused: `client_secret_post`.** §2.3.1 makes Basic the MUST and the
  body's credentials a MAY, "NOT RECOMMENDED". The guard never reads the body,
  so a client sending its secret there is answered as `None`: `400
  invalid_client`, §5.2's "unsupported authentication method".
- **Refused: a guard per method or per endpoint** (`ClientSecretBasicGuard`,
  `TokenEndpointGuard`). A further method is a `TokenEndpointAuthMethod` the
  same guard reads, and revocation (RFC 7009 §2.1) authenticates the client by
  §2.3, as introspection (RFC 7662 §2.1) may, so those endpoints bind this
  guard.
- **`#[public]` opens nothing**: every client authenticates at a token
  endpoint (§3.2.1), and no public client's grant is built.
- **The token request's own refusals** are §5.2's `invalid_request`, its
  `error_description` the located sentence the edge says, kept to §5.2's
  character set and never the value: a parameter missing, empty (§3.2, "treated
  as if they were omitted"), repeated or of the wrong kind, and a body that is
  no form (§4.4.2 fixes the encoding; the JSON `415` checkpoint 14b expected is
  superseded). `AccessTokenRequest` is its own extractor, documented as a form
  body by its type, and the request's one reader: it derives no `Deserialize`,
  so a `Form<AccessTokenRequest>`, which would skip each of these refusals,
  does not compile. What is refused before a parameter is read keeps the edge's
  answer: the cap's `413`, the router's `405`, the throttler's `429` (§2.3.1
  asks for the protection, not its shape).
- **Several mechanisms in one request** — §5.2's `invalid_request` for one that
  "utilizes more than one mechanism for authenticating the client" — is
  answered by the extractor, the one reader of the body: a `client_secret`,
  `client_assertion` or `client_assertion_type` (RFC 7521 §4.2) with a value,
  beside an `Authorization` header. A body `client_id` alone identifies and
  does not authenticate; it is ignored, as §3.2 ignores what the request does
  not define.
- **Not covered:** the methods `none` (a public client, which needs the
  authorization-code grant, not built), `client_secret_jwt` and
  `private_key_jwt` (RFC 7523), `tls_client_auth` and
  `self_signed_tls_client_auth` (RFC 8705), each a value the guard reads once a
  registry carries it; RFC 7009 revocation and RFC 7662 introspection, built
  nowhere; `/login`, a JSON `LoginDto` with no `grant_type` and no client
  authentication, which is no §4.3 token request, so its body refusals stay the
  edge's problem documents; and the OpenAPI document, which still lists a
  guard's declared statuses as problem documents, a `401` with a `Bearer`
  challenge and a `bearerAuth` requirement.

## 2026-10-29 — the grant's own refusals, and the registry, are the server's

The owner agreed on 2026-10-28 that the identity audit's breaking changes land
in 7.0: an authorization server built on `nest-rs-oauth-server` wrote by hand
what RFC 6749 fixes — a `grant_type` compared to a literal, §3.3's scope
narrowing, the registered clients under its own config, their Basic
authentication — so the guard and the request taken above now carry the
grant's refusals too.

- **Taken: `GrantType`, read off the IANA OAuth grant-type registry**
  (`client_credentials` today). `grant_type` had stayed a `String` so the
  issuer could answer an unknown grant itself, since an enum under serde would
  have made it a deserialization error the issuer never sees. That argument
  fell with the extractor above: the request refuses a grant the endpoint does
  not serve as §5.2's `unsupported_grant_type` before the handler runs. The
  registry's values are exact strings, so `Client_Credentials` is unsupported,
  not a spelling.
- **Taken: `AuthenticatedClient::scope_for`, one strict §3.3 rule** — none
  asked is the whole registration, a scope outside it refuses the request
  whole as `invalid_scope`, a repeat is granted once. §3.3 lets a server grant
  part of what was asked; a quiet partial grant is a policy, and the framework
  carries no policy, so the one rule it ships refuses rather than narrows. A
  product wanting another rule reads `request.scope` itself.
- **Taken: the registry is `OAuthServerConfig<P>`** under
  `<PREFIX>_OAUTH__SERVER__CLIENTS`, the family member's own namespace; the
  demo's `<PREFIX>_OAUTH__CLIENTS` sat one letter from `OAUTH__CLIENT__*`, the
  opposite role. The boot refuses a registry that serves no token or the
  wrong one: none, a client with no resource, a duplicate or empty id, an
  empty secret (an empty Basic password would match it).
  `ClientSecretBasicStrategy<P>` is §2.3.1's Basic pair checked against it, the
  guard's ordinary strategy.
- **Each refusal of the grant files one `warn`** — the request's
  `unsupported_grant_type`, `scope_for`'s and `audience_for`'s — `token
  request refused` on `nest_rs::oauth::server`, its `reason` the wire code;
  the client is the line's `actor_id`, never a field, and what it asked for —
  the grant, the scope, the resource — is never quoted.
- **Refused: a policy hook in the registry** (a closure deciding partial
  grants, a product payload validator). The product's own checks over the
  registry — a scope outside the set its rules gate on — run in its own
  module-init hook, in the config's words; a hook for a hypothetical user is
  not built.
- **Not covered:** the grants the endpoint does not serve (`authorization_code`
  at this server, `refresh_token`, the device and JWT-bearer URNs), which
  `GrantType` gains as each is built, with `unauthorized_client` and
  `invalid_grant` raised by whichever reads them.

## 2026-11-05 — the document reads what the wire answers, and the registry's shape holds

On the owner's mandate of 2026-11-05. Each seen run first.

- **The OpenAPI document is closed from the 2026-10-26 entry's *Not covered*.** `/token`
  promised problem documents and `bearerAuth`, so a generated client mistyped every
  refusal and Swagger UI sent the wrong scheme. `Guard::denial_document`, a provided method
  whose default is what a `Denial` renders, declares the scheme a `401` challenges with and
  the body the guard answers instead; `ClientAuthenticationGuard` declares `Basic` and
  `TokenError`, which implements `JsonSchema` by hand, its codes held to its variants by a
  unit test. The guard says two facts a generator reads, and no route restates them. Its one
  neighbour, the `server_error`
  a handler answers at `500`, is `#[api(error(500 = TokenError))]`, the existing seam for a
  body a handler writes; a guard does not declare `500` (`Guard::denial_statuses`).
- **A guard declaring `401` on a `#[public]` route keeps the route's requirement**: `#[public]`
  opens nothing here (§3.2.1), and `security: []` said it did.
- **`RegisteredClient::client_secret` is `pub(crate)`.** Nothing outside the crate reads it, and a
  client with no secret (`none`, `private_key_jwt`, `tls_client_auth`, each named above) would
  otherwise need it `Option`, a break, or an empty string as a state, which the boot refuses.
- **The boot refuses a client's scope outside §3.3's grammar**, naming the client and never the
  scope: `"a b"` was granted whole when no scope was asked and refused when asked by name.
  The resource server's `scopes_supported` already held the same grammar.
- **Not covered:** `OAuthClientConfig::scopes` and each social provider's, which a provider
  reads and this server never grants; a refusal there would refuse what works today, so it
  waits for a caller it harms. A route reading `AccessTokenRequest` with no
  `ClientAuthenticationGuard` documents its `400` as a problem document while the request answers
  §5.2's body: every client authenticates at a token endpoint, so no such route is taught.
