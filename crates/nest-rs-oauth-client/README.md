# nest-rs-oauth-client

OAuth 2.0 client for NestRS: the Authorization Code flow with PKCE, a signed CSRF/PKCE transaction, and the token exchange — the role RFC 6749 §1.1 calls the client.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features oauth-client
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/security/authentication/oauth2/) · [GitHub](https://github.com/YV17labs/NestRS)
