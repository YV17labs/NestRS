# nest-rs-redis

Redis as one integration home: `RedisModule` opens the one shared connection (`<PREFIX>_REDIS__*`) to one Valkey server, a Sentinel deployment's primary or a Cluster — the URL's scheme says which, `rediss…://` for TLS, always verified — and one binding per port sits beside it — `RedisQueueModule` (the job queue on Redis Streams: the portable `dyn JobProducer`, and the consumer the queue port's worker runs), `RedisThrottlerModule` (the rate-limit store shared across replicas), and `RedisScheduleModule` (the occurrence lock a scheduled job declared `replicas = "one"` claims through, so each occurrence fires on one replica).

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features redis                # connection + queue
cargo add nest-rs --features redis-throttler      # cross-process rate limiting
cargo add nest-rs --features redis-schedule       # scheduled jobs that fire once across replicas
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/queue/) · [GitHub](https://github.com/YV17labs/NestRS)
