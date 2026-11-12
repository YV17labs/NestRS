# nest-rs-codegen

Shared token helpers for NestRS decorators. A third-party `proc-macro` crate
uses its root items — `Grammar` and `Arg` to read a decorator's keys,
`Refusals`, `duration_millis`, `pascal_case`, `snake_case`, the attribute
takers and `reroot` — which semver covers; the rest is the framework's own and
no API.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs-codegen
```

Depend on `nest-rs-codegen = "7.N"` from a `proc-macro` crate only; an app or
a library names the umbrella, `nest-rs`, and nothing else. What a decorator
crate emits names documented items alone, rooted through `reroot`
([Writing a package](https://nestrs.dev/semver/#writing-a-package)).

[Documentation](https://nestrs.dev/decorators/) · [GitHub](https://github.com/YV17labs/NestRS)
