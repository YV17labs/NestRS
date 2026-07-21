# nest-rs-testing

In-process testing harness for NestRS: boot an app's real DI graph and drive its HTTP/GraphQL/MCP surfaces with poem's TestClient, with provider overrides and no socket bound.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at `1.0.0` in lockstep, under a semver contract: breaking changes wait for `2.0`.

```toml
[dependencies]
nest-rs-testing = "1.0"
```

[Documentation](https://nestrs.dev/testing/) · [GitHub](https://github.com/YV17labs/NestRS)
