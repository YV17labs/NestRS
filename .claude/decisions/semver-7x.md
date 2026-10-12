# 7.x keeps what a program or an operator matches

Written on 2026-10-11, before 7.0's first publish: what 7.0 exposes, every 7.x
must keep, so the promise is written before anyone builds on it. Until then the
repository said three things — `__private` is outside semver, a consumer names
the umbrella alone, and "breaking changes wait for the next major" — and none
said what counts as API beyond Rust items, what a minor may change, or how a
package depends on nestrs.

**API is what a program or an operator matches**, and a break to any of it
waits for 8.0:

- **Rust items in tier 1** (`visibility-tiers.md`): every item reachable by a
  path crossing no `__private` module, no `__`-prefixed item and no
  `#[doc(hidden)]` item, in every published crate. `nest-rs-cli`'s library is
  the exception — its modules are `pub` for its own binary and suites — and
  its command line is API in its place: every command and flag. What `nestrs
  new` and `nestrs generate` write follows the release.
- **Decorator grammar**: every key and its meaning.
- **Configuration**: every `<PREFIX>_<NAMESPACE>__<KEY>`, its spellings and its
  default, and every `#[config]` field name.
- **Wire and storage**: the queue `Envelope` and its `WIRE_FORMAT_VERSION`, the
  problem-details body, the WebSocket envelope, every `nestrs:` key
  (`queue.md`), and every header the framework reads or writes — among them
  the W3C's trace context and `Server-Timing`, the IETF's `Deprecation`,
  `Forwarded`, `Link`, `Retry-After` and `WWW-Authenticate`, and the proxy
  conventions read behind a trusted peer — since a client, a proxy or a
  collector matches each by its name.
- **Observability names**: span targets, unit names, the operation line's
  field names and outcome words, and the words a field such as `cause` takes.
- **The umbrella's feature names.**

**A new refusal is a break**: one that turns a compiling program into a
compile error, or an app or a configuration that booted into a boot refusal —
a value a variable accepted, a composition the container wired. An upgrade
inside the line must leave what ran running.

Message prose is outside it — a compile error, a boot refusal, a log message —
so a misleading sentence is fixed in a patch, and a program matching prose
matches what nobody promised it. The list holds only what review can check on
a diff.

**A minor adds; a patch fixes.** A minor may add a tier-1 item; add a variant
to a `#[non_exhaustive]` enum, or a field to a `#[non_exhaustive]` struct or to
one with a private field; add a field to a `#[config]` struct or to a `Default`
struct nested in one; add a decorator key, a variable, a feature or a unit; add
a name to `nest_rs::prelude`; raise a public dependency's minor
(`public-dependencies.md`); and raise the MSRV, named in the changelog. A patch
fixes behaviour and may change anything in tier 2; it never raises the MSRV,
since a project held to its toolchain takes patches blind, and tokio raises its
own in a minor for the same reason.

**A `#[config]` struct grows by declaration.** The documented construction is
`X { …, ..Default::default() }`, so a new field is a minor; a literal naming
every field is not covered, and the semver page says so. This is weaker than
Cargo's SemVer reference, which counts a new field on an all-`pub` struct as a
major, and it is the one place the policy departs from it.

**Tier 2 is outside semver.** It may change in any release and is safe only
because every framework crate requires its siblings at `=` the release
(`manifests-ci.md`). A third party never names it, directly or through
`::nest_rs::<concern>::__private`.

**A package depends on the umbrella, at the minor it builds against, with no
default features**: `nest-rs = { version = "7.N", default-features = false,
features = [...] }`. The umbrella's defaults turn on `http` and `config`, and
Cargo's features only add up, so a package taking them would compile HTTP into
every app linking it, a headless worker included. Two exceptions: a
`proc-macro` crate depends on `nest-rs-codegen` and uses its root items alone,
emitting no `__private` path — a package built against one 7.x runs in an app
on a later one, where tier 2 may differ — and a package's tests take `nest-rs`
with `testing` as a dev-dependency.

**Third-party libraries** are classified in `public-dependencies.md`: a
public dependency's major moves only with a nestrs major, and every private one
is wrapped.

**Held by** the tiers (the compiler, then review for a stray
`#[doc(hidden)]`), `#[non_exhaustive]` where a type will grow, and review of
each change against the list above. Once 7.0 is published, `cargo-semver-checks`
against the last 7.x release adds a mechanical rung; before then there is no
baseline to compare. It reads `#[doc(hidden)]` items as outside the API, which
is tier 2 — verified, together with the allowance a `#[config]` struct needs,
when it is wired.

**Refused:**

- **Cargo's reading for `#[config]` structs**, a new field being a major. Every
  config would freeze for the whole line, or move behind a builder, which the
  one seam of `config-one-seam.md` refuses.
- **`#[non_exhaustive]` on a `#[config]` struct.** Rust then refuses the struct
  literal outside the defining crate, `..Default::default()` included, and that
  literal is how `for_root(x)` is written.
- **Covering message prose.** A reworded refusal would be a major, so a sentence
  that misleads would keep misleading for the whole line.
- **A package naming `nest-rs-*` sub-crates.** The umbrella's features are the
  install contract `just lint` builds one by one; a sub-crate is a compilation
  unit, and its tier 2 is exactly what moves under a package's feet.

## Open — the owner's

Whether a security fix that refuses a configuration 7.x used to accept — an
insecure one, booted before the fix, so a break by the rule above — may ship
inside 7.x. Recommended: yes,
in a patch, under a `### Security` changelog heading with a line in the
upgrading guide's 7.x notes: security comes first and "safe by default" cannot
wait for 8.0, while the hard "no" on breaks stays absolute for items, grammar,
names and wire formats. The answer changes how `CLAUDE.md`'s "no breaking
change inside a major" reads, so the policy holds no such clause until the
owner gives it; until then such a fix is the owner's to schedule, never shipped
in a 7.x on a session's judgement.
