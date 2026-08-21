# nest-rs-oauth-resource

OAuth 2.0 Protected Resource Metadata (RFC 9728) for NestRS: the discovery document, and the interceptor that stamps the resource_metadata pointer onto every 401 across HTTP, WS and MCP.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features oauth-resource
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/security/authentication/split-deployment/) · [GitHub](https://github.com/YV17labs/NestRS)
