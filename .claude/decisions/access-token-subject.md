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

## 2026-10-31 — the RFC 9068 §5 check stays the product's, in its words

The registry moved into the framework (`OAuthServerConfig<P>`,
`token-endpoint-client-refusal.md`), so "the OAuth configuration refuses a
registered `client_id` a UUID parses" now names the framework's config,
while the scheme deciding that a UUID is a user is the product's `Subject`.
The audit's proposal of a per-client validation seam in the registry is
refused, as `token-endpoint-client-refusal.md` refused a policy hook there: the
product's check runs in its own module-init hook over the registry and names
it in the config's words (`OAuthServerConfig::<P>::NAMESPACE`), and `nestrs g
auth`'s `Subject` says where such a check goes rather than pointing at a
registry that does not run it. A framework-owned `Subject<U>` would put the
product's claims scheme in the framework (`architecture.md`, *The product's
own*).

## 2026-11-04 — who decides a client's post write, and where the audit row goes

Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)", the identity audit's
second pass). `PostAuthorGuard` no longer answers a client's create: the ability
does, its `Create` grant on posts declared only under `actor.sub.user_id()`. It still
answers a client's publish, whose one posture slot is
`#[authorize(Update, bind = PostsService)]`, and a capability would drop the load and
the mask; a client may still update, so `Update` cannot carry the rule.

Chat's rule is the capability this file's rule would have named from the start,
`#[subject] Posting`, granted under the same `user_id()`; `ModerationGuard` is gone
(`subject-no-table-holds.md`).

The publication audit row leaves `insert_unscoped`, whose bar is pre-principal
provisioning and principal-less work, for `Creatable`: users are granted `Create` on
the row with `actor_id` pinned to their own id, so the grant fills the actor and the
re-check holds it (`create-fills-what-the-grants-pin.md`), and no guard, newtype or
parameter threads it. `current_actor_id()` stays the audit identity for a line, a
string the row's column would have to parse.

## 2026-11-05 — `sign` takes a principal, so a token names its subject

Reopens the 2026-10-25 entry's first refusal, settled by the owner's mandate of
2026-11-05, which covers the public API break. "The claims type holds `sub` by its
field's type" held only for a type that has the field: `C: Serialize` holds nothing,
and a claims type without one minted `at+jwt` with no `sub`, in silence. Seen run.

**`sign` and `sign_for` take `C: Serialize + PrincipalIdentity`.** A claims type already
says who it names, to the guard, through `#[principal(actor)]`, so the signer asks it
rather than reading a convention off serialized names, which is what the 2026-10-25
entry refused. A `sub` the claims carry as a string is kept, as every claim is; one
absent, `null` or blank is their `actor_id()`, and claims whose actor is `None` or blank
mint nothing. **A `sub` of another JSON type is a `SignError`**, quoting nothing they
hold: RFC 7519 §4.1.2, which RFC 9068 §2.2 cites, makes it a string, and this crate's own
`verify::<{ sub: String }>` refused what `#[principal(actor)] sub: u64` minted. Seen run.
It is not rewritten to the actor's string, since a claim the claims carry is never
overwritten and a type then disagrees with the token it mints.
`sign_handshake` keeps `C: Serialize`: a flow's transaction names no one.

That entry's other reason stands: a test minting a hand-made claim set, the one that
proves a verifier refuses it, needs a signer that signs it. `TestApp::bearer` signs what
it is given, through `nest_rs_authn::__private::sign_unchecked`, and so does a suite
forging one by hand. An application does not reach either.

Everyday code does not move: a claims type an app verifies already derives the trait,
and one an issuer only signs gains a derive and an attribute.

- **Refused: a second method beside `sign`** (`sign_principal`): two ways to mint, the
  old one still minting nothing.
- **Refused: comparing a carried `sub` with `actor_id()`.** The derive reads the one from
  the other, so only a hand-written impl can disagree, and it says so visibly.
- **`client_id` stays `Option`**: the 2026-10-25 entry's gap, the demo's user tokens naming
  none, is unchanged, and the JWT page now says so.

## 2026-11-07 — every registered claim is held to its JSON type, as `sub` is

The 2026-11-05 entry held `sub` to RFC 7519 §4.1.2's string and left the rest as written:
`client_id: u64` and `iss: u64` minted, and a verifier reading either as its type refused the
token, the defect that entry closed for `sub`. Seen run (`client_id: 4242` and `iss: 4242` both
signed). Settled by the owner's mandate of 2026-11-07, which covers the break.

**The signer holds each claim RFC 7519 §4.1 registers, and each RFC 9068 §2.2 adds, to the JSON
type its specification gives it**, on `sign`, `sign_for` and `sign_handshake`, refusing with a
`SignError` that names the claim and quotes nothing: `iss`, `sub`, `jti`, `client_id` (RFC 8693
§4.3) and `acr` strings; `exp`, `nbf`, `iat` and `auth_time` NumericDates, never negative, as
the verifier this crate pairs with reads none before the epoch; `aud` a string or an array of
them; `amr` an array of strings; `scope` (RFC 8693 §4.2) RFC 6749 §3.3's list. The table is one
private type, `RegisteredClaim`, so the next registered claim is one row.

- **`scope` accepts the empty string**, which `scope::space_delimited` writes for an empty list
  and every reader takes for a token granting nothing; §3.3's grammar needs one token, but
  refusing it would turn a client registered with no scope into a `500` at its first token.
  Dropping it instead is refused: a hand-written principal reading an absent `scope` as `None`
  would widen a token granting nothing into one no scoped rule applies to.
- **A registered claim serialized as `null` is left out**, as the five the signer stamps already
  were: a `null` names nothing, and a verifier reading the claim as its type refuses it.
- **Refused: rewriting a claim into its type** (a number into its decimal string): a claim the
  claims carry is never overwritten, and the type would disagree with the token it mints.
- **Refused: holding a value beyond its JSON type** — a StringOrURI holding a `:` to be a URI,
  `amr` to RFC 8176's registry: RFC 7519 §4.1 leaves the value to the application, and the
  verifier's refusal this entry closes is of the type alone. `roles`, `groups` and `entitlements`
  (RFC 9068 §2.2.3.1) are left out: RFC 7643's multi-valued attributes fix no one JSON shape.
- `sign_unchecked`, a suite's forger, still signs what it is given.

## 2026-11-07 — the demo's user tokens name the first-party client, and the signer requires none

The 2026-10-25 entry left the demo's user grants naming no `client_id`, which RFC 9068 §2.2
requires, until an authorization-code grant gave them a client. Seen run: `issue`, which `/login`
and the social callback call, minted `client_id` absent. Settled by the owner's mandate of
2026-11-07.

- **The demo's first-party login names its own client**, `SignInConfig::client_id`
  (`<PREFIX>_SIGN_IN__CLIENT_ID`, `publish-web` by default): the Publish web client the sign-in
  serves, which it does not invent but is deployed for. The boot refuses a registered OAuth
  client holding that id (`names_the_first_party_client`), so a user's token never reads as a
  machine client's. `features::Claims::client_id` is a `String`: every demo token names a
  client, so the type says so, and a literal cannot leave it out.
- **The signer does not require `client_id`.** The 2026-11-05 entry made `sub` impossible to
  forget at no cost because every claims type already names its actor for the guard; nothing a
  claims type already says names its client. Each way to require one costs a line in every app
  minting tokens, weighed against the developer-first rule and left to the owner (`TODO.md`):
  a refusal in `sign` asks `nestrs g auth`'s claims, its dev token and every test's claims literal
  for a client, which the template's development route would have to invent; a
  `#[principal(client)]` role asks the same field and an attribute; a client argument on `sign`
  asks it at every call. **Refused: a configured default stamped when the claims name none**: a
  machine grant that forgot its `client_id` would go out as the first-party client's, the
  confusion RFC 9068 §5 asks an issuer to prevent, where a forgotten field should fail.
- The same holds for `iss` and `aud`, stamped only when configured: requiring them asks every
  signing deployment for two variables, the owner's call too.

## 2026-11-11 — every token names its client, its issuer and its audience

The 2026-11-07 entry left `client_id`, `iss` and `aud` optional at the signer, each a line in
every app minting tokens, and the owner's call. Seen run: `sign` minted `{exp, iat, jti, sub}`
for claims naming no client, a service configured with neither issuer nor audience started and
minted a token naming neither, and a verifier naming neither accepted a token naming neither.
Settled by the owner's word of 2026-11-11 (every line `TODO.md` lists, security first), by the
first value: RFC 9068 is followed, never improvised, and the developer's code is kept easy by
what the scaffold writes rather than by leaving the claims out.

- **`sign` and `sign_for` refuse claims naming no `client_id`** — absent, `null` or blank — with
  a `SignError` naming the claim, read off the payload the signer is about to mint: nothing a
  claims type already says names its client, and RFC 9068 §2.2 requires one. `sign_handshake`
  names no one, and `sign_unchecked` forges what it is given. The check runs after the
  subject's, so a claims type naming neither is told of its subject first.
- **A `JwtService` names an issuer and an audience, or does not start**, whether it signs or
  only verifies, the refusal naming each setting it lacks; a blank one is unset. §2.2 has
  every token carry both, and §4 has a resource server refuse one naming another or none. A
  verifier naming no audience already refused every token carrying one (jsonwebtoken refuses
  an `aud` it was given none to compare), so once the signer stamps both always, such a
  verifier would answer `401` to every request: the boot is where to learn it. The check runs
  last in `JwtService::new`, after the key's own refusals.
- **A bound service keeps reading both off its resource** (the 2026-10-31 binding): under
  `OAuthResourceModule` the audience is the resource and the issuers its authorization
  servers, so a verifier there sets neither and boots. One minting access tokens names its own
  `ISSUER`, or `sign` and `sign_for` refuse naming it, while its handshakes, which name no
  access-token issuer, sign as before. `ALLOW_ANY_AUDIENCE` stands in for the audience at the
  start, and such a service mints with `sign_for` alone: `sign` refuses, its token naming no
  audience. These two refusals sit at `sign`, not at the boot, since an HMAC secret signs
  whatever the app means it for, and a resource server sharing one may mint nothing.
- **The developer writes nothing more**: `nestrs g auth`'s `Claims` holds `client_id: String`,
  as the demo's does; its development token names its route, `dev-token`; `nestrs new` writes
  each app's issuer and audience, its own origin, beside its port in `apps/<app>/.env`, read
  under the development and test profiles, the lines unread until the app authenticates. A
  deployment sets its own, as it sets the port. A hand-written app learns at boot or at
  `sign`, never from a client.
- `nest_rs_authn::__private::forger` builds a suite's signer, held to everything `new` holds
  but the issuer and the audience: `TestApp::bearer` forges with it what an app must refuse,
  stamping the app's issuer and audience where it names them.
- **Refused, standing**: a configured default `client_id` stamped when claims name none (the
  2026-11-07 entry's reason). **Refused**: an issuer or an audience the framework derives from
  the listen port when unset — a deployment's origin is its public URL, which no port says, and
  a default would mint tokens naming `localhost` in production; a `#[principal(client)]` role,
  an attribute on a field the payload check already reads.
