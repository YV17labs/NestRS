# nest-rs-core

Core building blocks for the NestRS framework: the DI container, the module trait, the lifecycle, the boot-time access graph, and the one pipeline every unit of work runs through, with its interceptors and exception filters.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs
```

The kernel every capability wires into — it ships with the umbrella
unconditionally, so there is no feature to enable and no reason to name this
crate in a manifest.

[Documentation](https://nestrs.dev/fundamentals/) · [GitHub](https://github.com/YV17labs/NestRS)
