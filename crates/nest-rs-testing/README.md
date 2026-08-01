# nest-rs-testing

In-process testing harness for NestRS: boot an app's real DI graph and drive its HTTP/GraphQL/MCP surfaces with poem's TestClient, with provider overrides and no socket bound.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add --dev nest-rs --features testing
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/testing/) · [GitHub](https://github.com/YV17labs/NestRS)
