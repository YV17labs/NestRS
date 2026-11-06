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

## 2026-10-17 — a presigned `PUT` binds its content type; its length waits

`presign_put` takes a `SignedPut` and folds its `content_type` into the SigV4
signature (`X-Amz-SignedHeaders=content-type;host`): the store refuses an
upload sending another type, or none, so a client can no longer store
`text/html` under a key the app serves. `content_type` is required — a caller
accepting any type writes `application/octet-stream`, a visible line — and a
value no header carries as sent is refused at the call, naming the argument.
Both presigns refuse a lifetime outside SigV4's `X-Amz-Expires`, 1 second to 7
days, which S3 would refuse only when the URL is used. `head` reads the stored
type, ETag and last write through `get_opts` with `head: true`, since
object_store's `head` keeps only `ObjectMeta`.

**Refused:**

- **Binding the length (`SignedPut::with_content_length`) in 7.0.** object_store
  0.14 refuses `content-length` among a presigned URL's signed headers
  (`RESERVED_SIGNED_HEADERS`) and leaves it out of every canonical request, so
  the field could only be a promise the store never checks. A size bound is
  read back with `head` once the upload lands. It is a minor once object_store
  signs `content-length` on a presigned URL, or a POST policy's
  `content-length-range`: `SignedPut` is `#[non_exhaustive]`.
- **Signing with a SigV4 implementation of our own, or a second signer beside
  object_store's** to reach the length: a signature improvised, or the
  bucket's URL built twice.

## 2026-11-06 — a key the store rejects refuses the boot; only a policy boots on

The reason recorded above for booting on a `403` or a `401` ("a least-privilege
policy without `s3:ListBucket` is correct") covers a policy, and the proof gave
it to every refusal: a revoked or mistyped key booted and read `up`, with one
`warn`, while every call failed. S3 tells them apart in its error document, so
the connector reads a refusal's, at most 64 KiB, and the `<Code>` decides.
`InvalidAccessKeyId`, `SignatureDoesNotMatch`, `InvalidSecurity` and any `401`
refuse the boot naming both credential variables, and read `down` on a probe.
`AccessDenied` boots on with the `warn`, and so does what names no key: a code
nobody listed, a gateway's page, no document. A boot is never refused over a word
this code cannot read. The `warn` says what is known, a `403` naming no rejected
key and most often a policy, with S3's code when a closed list holds it, so
`RequestTimeTooSkewed` is not sent to read a policy. A read of the document that
fails is what it is, an interruption: the proof retries it, since a document cut
before `</Code>` may have named a rejected key.

object_store then meets an empty body: it quotes what it meets in its error's
sentence, and this document echoes the key and the request signed. A refused boot
reaches a `main` returning it, which prints each link's own `Display`, past every
`SafeLink`, so `Refused` and `Unreachable` hold object_store's error as the
sentence `error_message` says it, never the error: its links also quote the
request's URI, and the document of a `400` or a `404`.

**Refused:**

- **Reading the code off object_store's error.** Its type is unreachable, as the
  status's is; the connector already stands where the answer is.
- **A second request to tell them apart** (a `HeadBucket`, a `GetObject` of a key
  that does not exist). Each costs every probe a request, and a policy may refuse
  those too.
- **An XML parser for one element.** The code is matched against a closed list,
  so what a parser would add is rejecting a malformed document that names no
  rejected key anyway.
- **Handing the document back to object_store.** Nothing there reads it for a
  `401` or a `403` but the sentence it writes, so all it adds is what a `{:?}` of a
  call's error would repeat.
- **Quoting the code as the store spelled it.** A line repeats a member of the
  list, never a word of the document.

**The limit.** A valid key whose policies were all removed still answers
`AccessDenied`, which nothing tells from least privilege: it boots, reads `up`,
and shows at its first call. A server speaking S3's API with a code of its own
for a rejected key reads as a policy's refusal until its code joins the list. A
call's error keeps object_store's as its source, by contract, so a `{:?}` of it
still quotes the request's URI, the key among it, and a document other than a
`401`'s or a `403`'s; only `error_message` says it without them.

## 2026-11-06 — a call's error holds the sentence too

The limit above is closed, and the contract it kept is what moved. A call's error
held object_store's as its source, and a test holding a marked key showed what that
prints: the object's path in a `{:?}`, the request's URI in each of the chain's
`Display`s, and a document other than a `401`'s or a `403`'s, whoever read the error
but `error_message`. Those errors now hold the sentence `error_message` says, as a
refused boot's already did. `downcast_ref::<object_store::Error>()` goes with it:
`source()` is what every chain reader walks, so no spelling keeps the library's error
there and out of their print. Its one everyday use, telling a key holding nothing
from a failure, is a variant, `StorageError::NotFound`, which takes no
`object_store` line in a manifest. `Init` still holds what the builder refused: no
key or request exists yet, and a test holding a marked URL, endpoint, secret and
material shows it quotes none.

**Refused:**

- **A wrapper holding object_store's error behind a redacting `Debug`, with an
  accessor to reach it.** It keeps a downcast nobody writes, at the price of an
  error holding the value and a manifest line on a private dependency.
- **A kind enum mirroring object_store's variants.** No caller decides on a
  `PermissionDenied` or a `Precondition`: an operator reads them in the sentence.

The lines follow: `key` leaves the abandoned upload's and the resumed download's,
which name the `store` and carry the trace. The operator who must reconcile a
dangling upload lists the bucket's incomplete uploads (S3's `ListMultipartUploads`)
or leaves them to its lifecycle rule; object_store could never list them anyway.
