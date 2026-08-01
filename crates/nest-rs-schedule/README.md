# nest-rs-schedule

Discovered cron-style scheduled jobs for NestRS: the #[scheduled] decorator plus a ScheduleModule that auto-attaches the Scheduler.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features schedule
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/schedule/) · [GitHub](https://github.com/YV17labs/NestRS)
