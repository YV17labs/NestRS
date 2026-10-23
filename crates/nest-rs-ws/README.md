# nest-rs-ws

WebSocket gateways for NestRS: a #[gateway] struct and its #[messages] impl self-mount on the HTTP transport's upgrade.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features ws
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/websockets/) · [GitHub](https://github.com/YV17labs/NestRS)
