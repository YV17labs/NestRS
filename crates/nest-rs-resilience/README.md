# nest-rs-resilience

Resilience for NestRS: `#[resilient]` runs a method calling a dependency under a named policy — a timeout per attempt, retries when it is idempotent, a circuit breaker read and switched by hand, a bulkhead and an outbound rate limit — answered 503 with Retry-After.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features resilience
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path.

[Documentation](https://nestrs.dev/fundamentals/resilience/) · [GitHub](https://github.com/YV17labs/NestRS)
