# nest-rs-resilience

Resilience policies for NestRS outbound calls: a per-attempt timeout, jittered retries for calls declared idempotent, a circuit breaker shared by every caller of a dependency, a bulkhead and a fallback. A refusal answers `TIMEOUT` (`504`), `CIRCUIT_OPEN` (`503` with `Retry-After`) or `BULKHEAD_FULL` (`503`) on every edge.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features resilience
```

```rust,ignore
use std::time::Duration;
use nest_rs::resilience::{CircuitBreaker, Policy, Retry};

let payments = Policy::builder("payments")
    .timeout(Duration::from_secs(2))
    .retry(Retry::DEFAULT)
    .circuit_breaker(CircuitBreaker::DEFAULT)
    .build()?;

// Retried, since the caller declares the call idempotent.
let receipt = payments
    .run_idempotent(|cancel| async move { provider.fetch_receipt(id, cancel).await })
    .await?;
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability.

[Documentation](https://nestrs.dev/resilience/) · [GitHub](https://github.com/YV17labs/NestRS)
