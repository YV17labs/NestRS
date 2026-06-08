# nest-rs-testing

In-process testing harness for nestrs: boot an app's real DI graph and drive its HTTP/GraphQL/MCP surfaces with poem's TestClient, with provider overrides and no socket bound.

Part of **[NestRS](https://nestrs.dev)** — an opinionated Rust framework for backend apps.

## Install

```toml
nest-rs = { version = "0.1", features = ["testing"] }
```

## Documentation

- [Testing](https://nestrs.dev/testing/)
- [Getting started](https://nestrs.dev/getting-started/)
- [Full documentation](https://nestrs.dev)

> **Alpha** — the public API is still evolving and this release is not production-ready.
