# nest-rs-static-files

Static files and single-page apps for NestRS, from a directory or a folder embedded in the binary, served beside the API as the router's fallback.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs --features static-files
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — `#[derive(Embed)]` names `nest_rs::static_files::rust_embed` as its `crate_path`.

[Documentation](https://nestrs.dev/http/static-files/) · [GitHub](https://github.com/YV17labs/NestRS)
