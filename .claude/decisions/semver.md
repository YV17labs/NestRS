# A major keeps what a program or an operator matches

Written on 2026-10-11, before 7.0's first publish: what a major's first release
exposes, every minor and patch of it must keep, so the promise is written before anyone builds on it. Until then the
repository said three things — `__private` is outside semver, a consumer names
the umbrella alone, and "breaking changes wait for the next major" — and none
said what counts as API beyond Rust items, what a minor may change, or how a
package depends on nestrs.

**API is what a program or an operator matches**, and a break to any of it
makes its release the next major:

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
default features**: `nest-rs = { version = "<major>.<minor>", default-features
= false, features = [...] }`. The umbrella's defaults turn on `http` and `config`, and
Cargo's features only add up, so a package taking them would compile HTTP into
every app linking it, a headless worker included. Two exceptions: a
`proc-macro` crate depends on `nest-rs-codegen` and uses its root items alone,
emitting no `__private` path — a package built against one minor runs in an
app on a later one, where tier 2 may differ — and a package's tests take `nest-rs`
with `testing` as a dev-dependency.

**Third-party libraries** are classified in `public-dependencies.md`: a
public dependency's major moves only with a nestrs major, and every private one
is wrapped.

**Held by** the tiers (the compiler, then review for a stray
`#[doc(hidden)]`), `#[non_exhaustive]` where a type will grow, and review of
each change against the list above. Once a release is published, `cargo-semver-checks`
against the last release of its major adds a mechanical rung; before then there
is no baseline to compare. It reads `#[doc(hidden)]` items as outside the API, which
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

**2026-10-23 — `OpenTelemetryConfig` grows as a config does.** It is read
before the container exists, so it is no `#[config]`, but a program builds it
over its base (`new`, `from_env`) as a config is built over
`..Default::default()`: a new field is a minor, a complete literal not
covered (`room-to-grow.md`).

**2026-11-04 — `McpIdentity` grows as a config does.** It travels in
`McpModule::for_root`'s value, so it is no `#[config]`, and an app sets it one way, in
a literal over `Default`, as `OpenTelemetryConfig` is: a new field is a minor, a
complete literal not covered. Agreed by the owner ("Tout faire (Recommandé)",
2026-10-31, the identity audit's second pass).

## Open — the owner's

Whether a security fix that refuses a configuration a major used to accept — an
insecure one, booted before the fix, so a break by the rule above — may ship
inside that major. Recommended: yes,
in a patch, under a `### Security` changelog heading with a line in the
upgrading guide's notes for that major: security comes first and "safe by
default" cannot wait for the next major, while the hard "no" on breaks stays absolute for items, grammar,
names and wire formats. The answer changes how `CLAUDE.md`'s "no breaking
change inside a major" reads, so the policy holds no such clause until the
owner gives it; until then such a fix is the owner's to schedule, never shipped
in a minor or patch on a session's judgement.

## 2026-10-12 — answered: a security fix refusing an insecure configuration ships in a patch

The owner's arbitration of 2026-10-12, written here on 2026-10-27, closes the
open question above. A security fix that refuses a configuration a major used to
accept — an insecure one, so a break by the rule above — is not a breaking
change inside that major: it ships in a patch, under the changelog's `### Security`,
with a line in the upgrading guide naming the refused configuration and the
secure one to write instead. Security comes first, and "safe by default"
cannot wait for the next major. The hard "no" on breaks stays absolute for everything
else — items, decorator grammar, names, variables and wire formats — and the
exception is read narrowly: the configuration it refuses is insecure by a
standard (an RFC, OWASP), never merely unusual.

## 2026-11-18 — an exported telemetry name promises what its convention does

The owner's answer of 2026-11-17, before 7.0's first publish. The list above
froze what an operator reads off the console — span targets, unit names, the
operation line's fields — and said nothing of what leaves through the
OpenTelemetry export: a span's name and attributes, the resource's, a metric
and the attributes it is recorded under, a log record's event name and
attributes. 7.0 exports names from semantic conventions still in development —
`enduser.id`, `messaging.*`, `process.*`, `db.client.connection.*`, the MCP
conventions' `mcp.*` and `gen_ai.*` — which their convention may rename before
it is stable.

**A name drawn from a stable OpenTelemetry convention is API; a name drawn from
a convention in development follows its convention, and its rename is a minor
named in the changelog** — never a patch, which a deployment takes without
reading it. It holds for what a span, a metric and the resource carry; a log
record keeps what its line keeps on the console (below). Decided with it, from
the rule above (API is what a program or an operator matches) and correctness
(an export never keeps a name against the standard a backend reads it by):

- **The status is read in the conventions release the workspace pins**, never
  from memory, a name the code writes by hand included:
  `opentelemetry-semantic-conventions` 0.33 generates release 1.42.0, the
  schema every export declares. There `http.request.resend_count` is stable,
  and the MCP conventions are in development, marked moved to OpenTelemetry's
  GenAI conventions repository, where the framework follows them.
- **A value a convention lists for a name takes the status the convention gives
  that member**, which may differ from its name's: in 1.42 `db.system.name` is
  stable, its `postgresql` stable and its `redis` in development, and
  `http.request.method`'s `QUERY` is in development. The crate generates no
  member, so a member's status is read in that release's model
  (`model/<area>/registry.yaml`).
- **A name the framework gives itself on a span, a metric or the resource is
  API**, as a unit name is: the tokio runtime's four metrics,
  `dropped_client_spans_count`, and the fields a unit's span carries beside its
  line's (`trace_id`, `ws.connection_id`, `backend_id`). So is a value it
  chooses for a convention's name, as `error.type`'s `timeout` and
  `tool_error`. One given under the namespace of a convention in development —
  `graphql.operation.role` and `graphql.field.name`, under GraphQL's — follows
  that convention as its own names do.
- **A log record keeps what its line keeps on the console**, and so does a
  line a span carries as an event: an operation line's event name, its unit's,
  and its fields, and the words a field such as `cause` takes. Any other
  line's event name (`event <file>:<line>`, tracing's default) and fields are
  outside the contract, like its prose.
- **A name a library adds on its own moves with that library**, in a minor
  named in the changelog: `tracing-opentelemetry`'s `code.*`, `thread.*`,
  `busy_ns` and `idle_ns` on every span it exports, the SDK's
  `telemetry.sdk.*` on the resource.

Held by review: a change exporting a name marks it on its `/opentelemetry/`
page, and a move of the conventions pin reads every mark again.

**Refused:**

- **Freezing every exported name until the next major**, one in development included: the
  dashboards a backend ships for a convention read the name it renamed to, and
  would stay dark for the rest of the line (standards over invention).
- **Exporting the old and the new spelling side by side until the next major**: two
  spellings of one fact on one span, which `actor-under-enduser-id.md` refused,
  and every metric series twice.
- **Exporting a name only once its convention is stable**: the messaging,
  process, pool and MCP conventions are all in development, and the backends
  read them today.
- **Keeping every log record's event name and attributes**: the framework
  files some 470 lines, whose fields no page lists, and a line's default event
  name moves with its file and line; a field would be kept over OTLP and free on
  the console, where the operation line alone is kept.

## 2026-11-18 — a release ships when it is ready

The owner's answer, before 7.0's first publish. `SECURITY.md` spaced majors six
months apart and announced each four weeks ahead; both go. Every release —
major, minor or patch — ships when it is ready, a major with its upgrading
guide. What a supported line receives is unchanged: the latest minor of the
current major every fix, the previous major security fixes for three months
after the current major ships, its end date written when it does.

**Refused:** keeping the six months as a floor. A fix that needs a major — a
public dependency's next major carrying a security fix, among them — would wait
on a date rather than on its readiness.

## 2026-11-25 — compatibility with a published release is no constraint

The owner's directive, one week after the current major's first publish: nobody
is bound to keep what a published release exposed; what is wanted is
performance and the best code, and where something better is found, the
published design is changed rather than worked around, plainly said, and a rule
that contradicts this is changed with it.

**The list above still says what a break is; it decides a release's number,
never a design.** A release holding a break is a major, and it ships when it is
ready (2026-11-18): a major line lasts until something better is found, no
longer. "What a major's first release exposes, every minor and patch of it must
keep" is read of that major's releases alone.

- **A considerable gain is taken whatever it breaks** — performance, safety, or
  an architecture with fewer cases — and is stated with its evidence: a measure,
  or the cases, features and limits that go. A gain nobody can state is taste.
- **A break is told, never asked**: the commit (`!`), the changelog's upgrading
  entry, the report.
- **It goes whole**: no alias, no shim, no second reading kept beside the new
  one. A detector for persisted data stays, and a migration dropping or rewriting
  data is still asked.
- **A refusal in these files whose only reason is a break inside a major is open
  again**, reopened by the change that takes it, with its own dated entry.

**Superseded:** the hard "no" on breaks inside a major, and "a break waits for
the next major" wherever this file and `manifests-ci.md` said it; `ROADMAP.md`'s
"the line lasts" and "the next major stays mechanical … no redesign rides
along".

**Refused:**

- **A break under a minor's number.** Cargo takes a minor without asking, so a
  program would stop compiling on `cargo update`: a number costs nothing, and
  SemVer is the standard the semver page promises to a reader.
- **A grace period, the old API deprecated beside the new until the next
  major.** Two ways to do one thing for as long as it lasts, and the worse one
  the one every program already spells.

## 2026-11-26 — unreleased work names no release number

The owner's directive, 2026-11-26: nobody knows the number the work on a branch
will ship under, so nothing says it. What is not yet released lives in
`changelog/unreleased.md`, its index row `Unreleased`, and its upgrading guide
is the page `/upgrading-unreleased/`; no file, page, decision, rustdoc, comment
or fixture names the release it will join, nor compares it to a published one
by number — it says "the published release", "before this change", or the
behaviour alone. Cutting the release numbers it, and renames the changelog file
and the page for that number. What a published release shipped keeps its number
(`changelog/7.0.md` and older, a dated entry describing it), and so does a
third-party crate's version.

This file was `semver-7x.md`, a name presuming the line it describes; it is
named for what it decides, and its rules read "a major", "a minor" and "the next
major" where they named the published line and the next one by number. The
2026-11-25 entry still decides the number a release holding a break takes: a
major. Held by review.

**Refused:** naming the line by the number it is expected to take until it is
cut. A number written ahead is a promise the cut may not keep, and every page,
decision and rustdoc naming it is one more edit when it moves, or one more
sentence that lies.
