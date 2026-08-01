# nest-rs-events

Typed in-process event bus for NestRS: emit a typed event, dispatch to discovered #[on_event] methods grouped under #[listeners].

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features events
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/events/) · [GitHub](https://github.com/YV17labs/NestRS)
