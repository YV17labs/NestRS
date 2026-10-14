# nest-rs-worker

Worker-execution bounds for NestRS: the timeout every job attempt and scheduled tick runs within, and the failure an attempt cut at it reports.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features worker
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Queue workers](https://nestrs.dev/queue/) · [Scheduled jobs](https://nestrs.dev/schedule/) · [GitHub](https://github.com/YV17labs/NestRS)
