# nest-rs-storage

Object storage for NestRS: an injectable `Storage` client (presigned PUT/GET, head, byte and streamed read/write, prefix listing) over the `object_store` crate. One URL names the store and its scheme picks the backend — `s3://<bucket>[/<prefix>]` for AWS S3 and the servers speaking its API (tested against RustFS over TLS), `memory:///` for development and tests — and the boot proves the bucket answers before the app serves. Beside `health`, `StorageHealthModule` gates the readiness and startup probes on one attempt of that proof, as `storage`.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under one [semver policy](https://nestrs.dev/semver/).

```sh
cargo add nest-rs --features storage
cargo add nest-rs --features storage,health   # the store's readiness indicator
```

Reached through the [`nest-rs`](https://crates.io/crates/nest-rs) umbrella: one dependency, one feature per capability. Adding this crate directly is supported but not the documented path — a decorator's expansion roots itself at `nest_rs::…`.

[Documentation](https://nestrs.dev/storage/) · [GitHub](https://github.com/YV17labs/NestRS)
