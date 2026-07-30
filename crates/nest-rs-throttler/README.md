# nest-rs-throttler

Rate limiting for NestRS: a per-route ThrottlerGuard reading a #[meta(Throttle)] override, over a fixed-window counter behind a pluggable ThrottlerStore trait.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs-throttler
```

[Documentation](https://nestrs.dev/rate-limiting/) · [GitHub](https://github.com/YV17labs/NestRS)
