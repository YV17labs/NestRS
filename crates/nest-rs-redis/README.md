# nest-rs-redis

Redis as one integration home: `RedisModule` opens the one shared connection (`NESTRS_REDIS__*`), and one binding per port sits beside it — `RedisQueueModule` (the portable `dyn JobProducer`, via apalis-redis), `RedisWorkerModule` (the `RedisWorker` consumer transport), and `RedisThrottlerModule` (the rate-limit store shared across replicas). The user-facing storage is Redis; apalis is an implementation detail.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features redis                # connection + queue + worker
cargo add nest-rs --features redis-throttler      # cross-process rate limiting
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/queue/) · [GitHub](https://github.com/YV17labs/NestRS)
