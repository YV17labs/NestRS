# nest-rs-redis

Redis-backed queue integration for nest-rs-queue (via apalis-redis): the QueueConnection producer, the QueueWorker consumer transport, and the #[processor] decorator re-export. The user-facing storage is Redis; apalis is an implementation detail.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs-redis
```

[Documentation](https://nestrs.dev/queue/) · [GitHub](https://github.com/YV17labs/NestRS)
