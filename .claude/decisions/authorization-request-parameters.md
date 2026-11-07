# An authorization request carries what the provider reads, under the provider's names

## 2026-11-07 — one seam the OAuth client and every social provider share

Settled by the owner's mandate of 2026-11-07 (close what `TODO.md` lists), which covers the
breaks. Seen run: the authorization request `OAuthClient::authorize` built for Google carried
`response_type`, `client_id`, `state`, `code_challenge`, `code_challenge_method`,
`redirect_uri` and `scope`, and no config could add a parameter. Google issues a refresh token
only to a request saying `access_type=offline`, and on a consent already given only with
`prompt=consent`, so a deployment calling Google on its users' behalf had none; GitHub's
`prompt=select_account` and `allow_signup` had no way in either. And the refresh token a
provider did issue — a GitHub App's expiring user token — ended in the strategy: `SocialSignIn`
was handed the profile alone.

- **`AuthorizationParameters`**, in `nest-rs-oauth-client`, read by `OAuthClientConfig` and by
  each provider's config under `AUTHORIZATION_PARAMETERS`, a JSON object of strings. Its keys are
  the parameters' own names — the OAuth Parameters registry's, OpenID Connect Core's, a
  provider's documentation — so a deployment copies them as the provider writes them; the request
  carries them after the client's own.
- **The boot refuses what the client owns**: `response_type`, `client_id`, `redirect_uri`, `scope`
  and `state` (RFC 6749 §4.1.1), `code_challenge` and `code_challenge_method` (RFC 7636 §4.3),
  `nonce` (OpenID Connect Core §3.1.2.1), and what would move the answer or the request where the
  callback reads nothing — `response_mode`, `request` and `request_uri` — in any case, since a
  provider may read a name loosely. Also refused: a name outside RFC 6749 §8.2's `param-name`, a
  parameter with no value (§3.1 treats it as omitted), and one named twice (§3.1 forbids it),
  which is why the JSON is read as written rather than into a map keeping the last.
- **`SocialSignIn::sign_in` takes the `TokenSet`** beside the profile, the shape Passport's verify
  callback has: the refresh token reaches the product that asked for it, to keep for its calls on
  the user's behalf.
- **Refused: typed fields per parameter** (`access_type`, `prompt`) on each config: the set is a
  provider's vocabulary, open, and a field per member would be a framework change for every one a
  provider adds.
- **Refused: a per-request seam** (a `login_hint` read off the request): no caller, and a request's
  own value in the authorization URL is the product's handler to choose.
- **Refused: a refresh grant on `OAuthClient`**: no caller in the framework or the demo, which
  signs users in and keeps no provider token; a product calling a provider holds its own HTTP
  client (`CLAUDE.md`, no outbound `HttpService`).
- `Debug` prints the names alone, since a value is the deployment's and costs nothing to keep out.

## 2026-11-08 — the client's credentials and verifier are refused too, and a name is one in any case

Seen run, by this branch's review: `{"client_secret":"…","client_assertion":"…","code_verifier":"…"}`
booted, and `authorize` wrote each into the authorization URL, the `Location` every browser is
handed — its history, a `Referer`, the provider's access log. RFC 6749 §2.3.1: client credentials
MUST NOT be included in the request URI. And `{"prompt":"consent","PROMPT":"none"}` booted, a
parameter sent twice on this entry's own premise that a provider reads a name loosely.

- **Refused as `sent_by_the_client` too: what the client sends the token endpoint alone** — its
  credentials, `client_secret` (RFC 6749 §2.3.1), `client_assertion` and `client_assertion_type`
  (RFC 7521 §4.2), and its PKCE verifier, `code_verifier` (RFC 7636 §4.5).
- **`named_twice` compares names in any case**, as `sent_by_the_client` does.
- **The list is the type's own, private**: public, a fixed-size array would put its length in its
  type, and the next member — DPoP's `dpop_jkt` (RFC 9449 §10), once the client binds its tokens —
  would break it. The type's documentation and the docs name the members.
