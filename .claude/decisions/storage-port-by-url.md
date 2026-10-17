# Storage is the capability's port, its object_store backend picked by URL

`nest-rs-storage` is the storage port, and its default implementation is
object_store, a library already multi-backend: `<PREFIX>_STORAGE__URL` names
the store, and its scheme picks the backend, as a Redis URL's scheme picks its
topology (`valkey-topologies.md`). 7.0 opens `s3://<bucket>[/<prefix>]` — AWS
and every server speaking its API — and `memory:///`, the in-process store a
development or test profile alone may open. Decided with 7.0's foundation
(2026-10-09); the sentence in `architecture.md` *Ports & Adapters* states it for
any port whose default is such a library.

The store is proved at boot through `nest_rs_core::prove` (one page of the
bucket's listing under the prefix). object_store reports no listing's status
through its error type, so the proof reads the answer's status through an HTTP
connector of its own: 404 refuses the boot, 403 and 401 boot on with one `warn`
(a least-privilege policy without `s3:ListBucket` is correct), a refused TLS
handshake refuses at once, the rest is retried within the operation budget.

**Refused:**

- **Renaming the crate `nest-rs-s3`.** An adapter is named for its vendor, but
  this crate is the capability's port — the market's word, Laravel's
  `Storage`, NestJS's `@nestjs/storage` — and `memory` is no S3.
- **A nestrs driver trait over object_store.** object_store already is the
  abstraction; a second one would carry no semantics of ours and wrap every
  backend twice.
- **`file://`, `gs://`, `az://` without a user.** No consumer asks; presigning
  does not exist on the filesystem; the transfer's `ETag`-fenced resume is
  unverified on a local filesystem. Each is a scheme learned in a minor.
- **`AmazonS3Builder::from_env`, `parse_url` and `parse_url_opts` on an
  `s3://` URL.** `from_env` reads every `AWS_*` variable,
  `AWS_ALLOW_INVALID_CERTIFICATES` among them, beside the namespace; the two
  parsers build the client from the URL and string options, past the explicit
  setters, `ClientTls` and the namespace's refusals. The URL is parsed here and
  the client built with explicit setters; `clippy.toml` disallows all three.
  Ambient AWS credentials, which object_store reads only through `from_env`,
  wait with them.
- **Classifying the probe by object_store's error variant.** A listing's 404
  arrives as `Error::Generic`, never `NotFound`; reading the status off the
  error's sentence would parse a dependency's wording.
- **Named stores (`Storage<I>`) in 7.0** — `named-instances.md`.

## 2026-10-17 — the registry joins the disallowed parsers

`object_store::registry::DefaultObjectStoreRegistry` resolves a URL it holds
no store for through `parse_url_opts` and every process variable,
`AWS_ALLOW_INVALID_CERTIFICATES` among them — a store opened past
`StorageConfig`'s refusals and `ClientTls`, as the two parsers above open one.
`clippy.toml` disallows the type beside them.
