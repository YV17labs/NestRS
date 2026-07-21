# nest-rs-throttler

Rate limiting for NestRS: a per-route ThrottlerGuard reading a #[meta(Throttle)] override, over a fixed-window counter behind a pluggable ThrottlerStore trait.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at `1.0.0` in lockstep, under a semver contract: breaking changes wait for `2.0`.

```toml
[dependencies]
nest-rs-throttler = "1.0"
```

[Documentation](https://nestrs.dev/rate-limiting/) · [GitHub](https://github.com/YV17labs/NestRS)
