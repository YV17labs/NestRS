# nest-rs-storage

S3-compatible object storage for nestrs: a thin, injectable `Storage` client (presigned PUT/GET, head, byte read/write) over the `object_store` crate. Multi-driver by design (S3/GCS/Azure/fs/memory); the AWS-S3 driver is wired by default and works against any S3-compatible server (AWS, MinIO, RustFS) in path- or virtual-host style.

[Documentation](https://nestrs.dev/storage/) · [GitHub](https://github.com/NestRS/NestRS)
