# nest-rs-resilience

Resilience for NestRS: named policies an app wraps around an outbound call — a timeout per attempt, retries for an idempotent call, a circuit breaker and a bulkhead — answered 503 with Retry-After.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features resilience
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path.

[Documentation](https://nestrs.dev/fundamentals/resilience/) · [GitHub](https://github.com/YV17labs/NestRS)
