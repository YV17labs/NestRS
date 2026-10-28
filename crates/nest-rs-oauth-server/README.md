# nest-rs-oauth-server

OAuth 2.0 authorization-server vocabulary for NestRS: RFC 6749 §2.3 client authentication at the token endpoint through a guard, §4.4.2's token request with RFC 8707's `resource`, §5.1's access token response and §5.2's token-endpoint error set.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features oauth-server
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

## What it holds

`ClientAuthenticationGuard<S>` authenticates the client at the token endpoint (§2.3), bound with `#[use_guards]` over the app's own authn `Strategy`, which it runs as `AuthnGuard` does. It refuses as §5.2 answers, on every route, `#[public]` included: `401 invalid_client` with the `Basic` challenge for a client refused over the `Authorization` header, `400 invalid_client` for one that sent no header — credentials in the body are not read — and `503 server_error` for a strategy that could not evaluate it. An unknown client and a wrong secret read the same bytes, and each refusal files one `warn` naming the method and the reason, never the client.

`AccessTokenRequest` is §4.4.2's request, read as a handler parameter off its `application/x-www-form-urlencoded` body and documented so — its only reader: it derives no `Deserialize`, so `Form<AccessTokenRequest>` does not compile. A request that does not parse — a parameter missing, empty (§3.2), repeated or of the wrong kind, a body that is no form, a credential in the body beside the `Authorization` header — is §5.2's `invalid_request`, its `error_description` saying where, never what it held. `resource` is the one parameter that may repeat (RFC 8707 §2): every one sent is read, in order.

`TokenError` is §5.2's closed set of six wire codes and RFC 8707 §2's `invalid_target`, rendered as the JSON envelope the section prescribes with §5.1's `no-store` / `no-cache`; the guard and the request answer through it. The response is marked `NoBearerChallenge`, so a token refusal is never dressed up as a oauth-resource challenge — a client authenticating with `Basic` reads the reason its credentials were refused, not a pointer to RFC 9728 discovery.

`AccessTokenResponse` is §5.1's success body: `AccessTokenResponse::bearer(token, expires_in)`, plus `.with_scope(granted)` whenever the issuer grants another scope than the one requested — a narrower set, or its default when the request named none — which §3.3 obliges it to state. A handler returns it bare, and it renders as its JSON with the same `no-store` / `no-cache`, which `Json<AccessTokenResponse>` would drop; its `Debug` redacts the token. A bare return names no body type for the OpenAPI document, so the route declares it with `#[api(response = AccessTokenResponse)]`.

`authenticate_against_registry` is the mirror of `nest-rs-oauth-client`: that crate presents credentials to somebody else's token endpoint, this one checks them at yours, from the strategy the guard runs. The comparison is constant-time and visits every entry whatever matched, so neither a valid `client_id` nor a secret prefix is observable by timing.

A `RegisteredClient` lists the resources its tokens may be minted for, and `AuthenticatedClient::audience_for(&request.resource)` reads the request's against them: the one requested when registered, the only one registered when none is requested, `invalid_target` otherwise — an unregistered resource, none among several, two at once. The issuer then mints with `JwtService::sign_for`, so the token's `aud` is that resource and no other resource server accepts it.

## Why it is not `nest-rs-authn`

That crate resolves *who is calling* at a resource server; this one answers a machine client at the authorization server, where the client is its own subject — `AuthenticatedClient::actor_id()` returns its `client_id`, as RFC 9068 §2.2 names it. `TokenError::UnsupportedGrant` and `TokenError::InvalidScope` are the tell: neither is a credential verdict. The two guards part the same way: `AuthnGuard` refuses a caller as RFC 6750 words it, `ClientAuthenticationGuard` a client as RFC 6749 §5.2 does, over the same `Strategy`. Keeping them together meant every resource server compiled a token endpoint it never serves.
