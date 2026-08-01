# nest-rs

NestRS framework umbrella — re-exports the core and the surface crates an app reaches for, with a `prelude` for the common case.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features http
```

One dependency. Each capability — `http`, `graphql`, `seaorm`, `ws`, `mcp`,
`queue`, `schedule`, … — is a Cargo feature, and a feature you leave off is
code you never compile. Undecided? `--features full` turns everything on.

```toml
[dependencies]
nest-rs = { version = "2.0", features = ["http", "seaorm"] }
```

[Documentation](https://nestrs.dev/) · [GitHub](https://github.com/YV17labs/NestRS)
