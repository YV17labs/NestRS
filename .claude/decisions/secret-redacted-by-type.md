# A secret is redacted by its type: `nest_rs::core::Secret`

A value no `Debug`, log line or serialized reply may print is a
`nest_rs::core::Secret<T>`: its `Debug` writes `<redacted>`, it has no
`Display`, it serializes as the string `"<redacted>"`, and `expose_secret()` is
the one read. Every secret the framework stores in a field is one — the
credential-bearing URLs, the storage pair, the pseudonym key, the JWT secret
and private key, every OAuth client secret, the tokens of an exchange, a TLS
private key, the personal data a social provider reports — and the types
holding them have a `Debug`. Under `nest-rs-seaorm` it is a column type over
its value's own, and `#[expose]` refuses it. Decided by the owner on
2026-10-28 ("Tout faire", the identity audit's feat-11): the hard "no" on a
secret in a log line is held by a type instead of review.

Until then the rule was held by hand, type by type: eight framework types
restated every field in a `Debug` of their own to hide one, a ninth printed a
key by its size, five more had no `Debug` at all — so no app struct holding
one could derive it — and the demo's user entity restated its nine columns in
two impls to hide a hash whose newtype serialized it raw. A field added beside
a hand-written impl printed its value until someone remembered the impl; the
third occurrence was long past.

**Serialization writes the placeholder, deserialization reads the value.** The
response mask serializes a row and keeps only its exposed keys, so a secret
column never reaches a reply either way; writing the placeholder makes a reply,
a stored document and a `validator` refusal — whose parameters serialize the
rejected value — hold nothing even where a key slipped through. No path
round-trips a row through serde to keep it: the mask's rebuild fills an
unexposed column from its placeholder and strips it again — for a `Secret`,
the value it holds, written over the `"<redacted>"` a raw `Model` carries,
which a byte-like value type cannot read back. A configuration decodes a
secret from its value, so deserialization reads it.

**A URL beside a secret is shown, not hidden.** An endpoint whose userinfo
or query may carry a credential — a provider's, a JWK Set's, a redirect —
stays a `String`, so its host and path can still be read in a log line; the
type holding it writes its `Debug` and shows it through `endpoint_shown`, as
the storage and OpenTelemetry configs already did. A URL that is a
credential as a whole — a database's or Redis's, carrying its password — is
a `Secret`.

**`==` is constant-time over the bytes.** sea-orm's `ActiveModel` derives
`PartialEq`, which needs it of every column; a type named `Secret` whose `==`
leaked where two values differ would invite exactly the comparison it must not
serve. It is defined for byte-like values (`AsRef<[u8]>`) through
`constant_time_eq`.

**The sea-orm column traits live in `nest-rs-core`, behind its `seaorm`
feature**, which `nest-rs-seaorm` enables: the traits are sea-orm's and the
type is the kernel's, so the orphan rule allows them nowhere else. sea-orm is
a public dependency already, and an app without `nest-rs-seaorm` compiles none
of it.

**`#[expose]` refuses by spelling** — a field whose type names `Secret` — since
a macro reads no type; an alias meets the wire DTO's `JsonSchema` derive, which
`Secret` does not implement, so it still fails to compile.

**Refused:**

- **secrecy.** Its last release, 0.10.3, dates from 2024-10-09: past the
  twelve-month bar.
- **Zeroizing on drop.** The values are handed to libraries as owned strings
  (sqlx, object_store, the JWT and OAuth clients) that never zeroize, so the
  wrapper would promise what the process does not keep; the hard "no" is about
  what is printed, stored and replied.
- **`Deref`, `AsRef<str>` or `Display`.** Each is a read that does not say it
  reads a secret; `expose_secret()` is the one place a review searches.
- **Failing serialization instead of writing a placeholder.** The mask
  serializes the whole row, so every reply of an entity with a secret column
  would fail closed.
- **`JsonSchema` for `Secret`.** It would let `#[expose]`'s DTO compile around
  an alias.
- **A second type for personal data.** A provider's email is kept out of a
  line for the reason a key is, and one concept takes one word.

**Limits — what the type leaves to review:** a secret passed through a
function boundary (`hash_password`'s return, `verify_password`'s and
`authenticate_against_registry`'s parameters) is a borrowed `&str` or a
`String` the caller stores into a `Secret` field; the type holds what is kept
in a struct, not every value in flight.
