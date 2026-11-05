# A suite signs its tokens with a key of its own

## 2026-10-31 — the app verifies a key the test holds, keeping its posture

`TestApp::bearer` signed with the app's own `JwtService`, which only an issuer
holds a private key for: on a resource server — every app of the demo but
`auth` — it panicked, so no suite called it, and the demo minted its tokens
with a committed copy of `auth`'s private key and a role passed as a string.
The owner agreed on 2026-10-31 ("Tout faire", the identity audit's second
pass) to the API break below.

`TestAppBuilder::ephemeral_signing_key()` issues an Ed25519 pair in process,
as `TestAuthority` issues certificates, and seeds it; `AuthnModule`'s factory
builds the app's `JwtService` over its public half in place of the key
material its configuration names, signing with its private half only where
that material signs, every other setting the app's own — its audience, its
issuer, the resource it serves. A configuration naming no key, the deployment's
to set, takes the public half and verifies only, whatever else it refuses still
refused: a resource server's suite commits no key material only for it to be
replaced. `bearer` takes a claims value and signs it
for that audience, as that issuer, stamping `exp` when the claims carry none;
a claim the claims carry is kept, which is how a test proves a refusal. The
boot proves the pair on one token, and refuses an app with no `JwtService`.
The WebSocket socket builder's `bearer(token)` went with it: `bearer` names
one thing, a claims value signed into an `Authorization` value.

**Refused:**

- **Installing the key whenever the harness's `authn` feature is on**: the
  workspace unifies that feature for every suite, so a framework test of a
  secret or a JWK Set would change behaviour with the feature set. The opt-in
  is one builder call.
- **Seeding an `AuthnConfig` read from the environment, its key replaced**:
  a seed supersedes a `for_root` pin, which would be dropped in silence.
- **Overriding `JwtService` with one the harness builds**: the resource
  binding that sets its audience and issuers is the authn crate's own.
- **Giving a verify-only app the private half**: it could mint in its suite
  and not in production.
- **A committed test key**: the `.env` cascade already holds the demo's, and a
  suite signing with it proves nothing a key of its own does not.
