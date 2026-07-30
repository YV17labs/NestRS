# nest-rs-queue

Backend-agnostic queue contract for NestRS: traits and the link-time #[process] registry a backend plugs into. Redis ships first-party in nest-rs-redis, and the same public seam carries any other store.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs-queue
```

[Documentation](https://nestrs.dev/queue/) · [GitHub](https://github.com/YV17labs/NestRS)
