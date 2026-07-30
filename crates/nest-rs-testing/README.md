# nest-rs-testing

In-process testing harness for NestRS: boot an app's real DI graph and drive its HTTP/GraphQL/MCP surfaces with poem's TestClient, with provider overrides and no socket bound.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add --dev nest-rs-testing
```

[Documentation](https://nestrs.dev/testing/) · [GitHub](https://github.com/YV17labs/NestRS)
