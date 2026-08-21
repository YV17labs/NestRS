# nest-rs-oauth-server

OAuth 2.0 authorization-server vocabulary for NestRS: RFC 6749 §5.2's token-endpoint error set, and §2.3.1 client authentication against a static registry in constant time.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features oauth-server
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

## What it holds

`TokenError` is §5.2's closed set of six wire codes, rendered as the JSON envelope the section prescribes with §5.1's `no-store` / `no-cache`. The response is marked `NoBearerChallenge`, so a token refusal is never dressed up as a oauth-resource challenge — a client authenticating with `Basic` reads the reason its credentials were refused, not a pointer to RFC 9728 discovery.

`authenticate_against_registry` is the mirror of `nest-rs-oauth-client`: that crate presents credentials to somebody else's token endpoint, this one checks them at yours. The comparison is constant-time and visits every entry whatever matched, so neither a valid `client_id` nor a secret prefix is observable by timing.

## Why it is not `nest-rs-authn`

That crate resolves *who is calling*, and an authenticated machine client is nobody — `AuthenticatedClient::actor_id()` returns `None`. `TokenError::UnsupportedGrant` and `TokenError::InvalidScope` are the same tell from the other side: neither is a credential verdict. Keeping them together meant every resource server compiled a token endpoint it never serves.
