# nest-rs-pipes

Transport-agnostic transformation pipes and validation for NestRS, applied by each edge between decoding an argument and its handler.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features pipes
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/fundamentals/pipes/) · [GitHub](https://github.com/YV17labs/NestRS)
