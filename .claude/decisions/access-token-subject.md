# An access token names its subject, and the signer writes what only it knows

## 2026-10-25 — RFC 9068 §2.2's required set, split between the signer and the claims

RFC 9068 §2.2 requires `iss`, `exp`, `aud`, `sub`, `client_id`, `iat` and
`jti` in a JWT access token, under the `typ: at+jwt` header of §2.1.
`JwtService::sign` stamped the header and the configured `aud` and `iss`, and
left the rest to the app's claims type; the demo, the `g auth` template and the
docs then minted tokens with neither `iat` nor `jti`, and the demo's
`client_credentials` tokens with no `sub` — so its machine clients, whose
subject is their own client id, could open no MCP session
(`mcp-session-bound-to-its-principal.md`).

**The signer writes the facts only it knows**: `iat`, when it signs, and `jti`,
a random v4 UUID drawn for that token alone, beside the configured `aud` and
`iss`. A claim the claims type carries is kept; one it serializes as `null` is
absent. **The claims type names who and for whom**: `sub`, `client_id`, `exp`.
Refused:

- **A runtime refusal in `sign` of claims naming no `sub`**: it reads a
  convention off serialized names, never a type, and a test minting a
  hand-made claim set — the one that proves a verifier refuses it — needs a
  signer that signs it. The claims type holds `sub` by its field's type.
- **A typed access-token wrapper around the app's claims**: every caller and
  every verifier rewritten for what that field type already holds.

## 2026-10-25 — the demo's subject is a user or a client, told apart by its shape

`sub` is one string, but a user's id and a client's id must never be read for
each other: a route writing a user's row reads a user. The demo's
`Subject` is `User(Uuid)` or `Client(String)`, read back by shape — a UUID is a
user. RFC 9068 §5 and RFC 9700 §4.15 ask the authorization server to keep a
client from choosing an id that collides with a user's subject, so the OAuth
configuration refuses a registered `client_id` a UUID parses, at boot, naming
the client. Refused: a second claim saying which kind (a token's two claims
could disagree, and every verifier would read both); prefixing the client's
`sub` (RFC 9068 §2.2 names the client's own identifier).

A machine token carries `client_id` beside `sub`. The demo's user grants — the
password login and the social callback — serve no OAuth client, none
identifying itself on them, so their tokens name no `client_id`: naming one
would invent it. That gap closes when the first-party login becomes an
authorization-code grant (RFC 6749 §4.1, with PKCE) by a registered public
client.

## 2026-10-25 — a client's post write is refused by its subject, whatever its scope

A post's author is a user, so no scope lets a client create or publish one.
RFC 6750 §3.1's `insufficient_scope` tells a client that a wider token would
serve it; naming `posts:write` to a client would send it after a token refused
all the same. `PostAuthorGuard` therefore answers a client's create or publish
`403 FORBIDDEN` before the scope gate, with or without `posts:write`, and a
user lacking the scope still gets the challenge. A write a client may do — an
update, a delete — keeps the challenge for the scope it lacks.

## 2026-10-31 — `exp` is the signer's too

The 2026-10-25 entry left `exp` to the claims type beside `sub` and
`client_id`, without a reason ranked above elegance. Unlike those two, `exp`
names no one: it is `iat` plus the lifetime the signer itself is configured
with, so every claims literal asked the signer for that value
(`JwtService::expiry()`) only to hand it back, and every test wrote `exp: 0` or
a closure passing it in. Decided on the owner's agreement to the identity
audit's second pass ("Tout faire (Recommandé)", 2026-10-31, its item 11),
which covers this public API break:

- **The signer stamps an access token's `exp` = `iat` + `expires_in`** when
  the claims leave it out or serialize it as `null`, under the rule above: a
  claim the claims carry is kept, so a test forging an expired token keeps
  its own. **The claims type names who and for whom**: `sub` and
  `client_id`. `JwtService::expiry()` is gone.
- **A handshake names its own lifetime**: `sign_handshake` stamps no `exp`
  and refuses claims carrying none, so a flow's token (the OAuth
  transaction's ten minutes, `expiry_in(secs)`) never inherits the
  access-token lifetime — one hour by default, up to thirty days — by
  omission. A handshake without `exp` failed closed before this entry, at
  verification, which requires the claim; it now fails at signing, naming
  `expiry_in`.
- `TestApp::bearer` takes the claims value it signs, as the closure existed
  only to pass `exp` in.

Refused: **an `exp` the claims type must still declare** (a field the
signer then overwrites): a value written twice, once ignored, is the drift
this entry removes.
