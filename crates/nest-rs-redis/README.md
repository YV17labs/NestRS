# nest-rs-redis

Redis-backed queue integration for nest-rs-queue (via apalis-redis): the QueueConnection producer, the QueueWorker consumer transport, and the #[processor] decorator re-export. The user-facing storage is Redis; apalis is an implementation detail.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at `1.0.0` in lockstep, under a semver contract: breaking changes wait for `2.0`.

```toml
[dependencies]
nest-rs-redis = "1.0"
```

[Documentation](https://nestrs.dev/queue/) · [GitHub](https://github.com/YV17labs/NestRS)
