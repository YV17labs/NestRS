# nest-rs-redis

Redis as one integration home for the framework's two cross-process needs: the queue (via apalis-redis) — RedisQueueConnection producer, RedisWorker consumer transport, #[processor] re-export — and RedisThrottler, the rate-limit store shared across replicas. The user-facing storage is Redis; apalis is an implementation detail.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features redis                # queue
cargo add nest-rs --features redis-throttler      # cross-process rate limiting
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/queue/) · [GitHub](https://github.com/YV17labs/NestRS)
