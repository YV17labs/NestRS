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
