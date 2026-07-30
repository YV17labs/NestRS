# nest-rs-storage

S3-compatible object storage for NestRS: a thin, injectable `Storage` client (presigned PUT/GET, head, byte read/write) over the `object_store` crate. The AWS-S3 driver ships wired and runs against any S3-compatible server (AWS, MinIO, RustFS) in path- or virtual-host style; the GCS, Azure, fs and memory drivers sit behind the same client.

Part of [NestRS](https://nestrs.dev) — every framework crate ships at the same version in lockstep, under a semver contract: breaking changes wait for the next major.

```sh
cargo add nest-rs-storage
```

[Documentation](https://nestrs.dev/storage/) · [GitHub](https://github.com/YV17labs/NestRS)
