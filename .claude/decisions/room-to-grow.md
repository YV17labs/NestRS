# Room to grow: what 7.x may add to a public type

Written on 2026-10-23, before 7.0's first publish. A 7.x adds a variant or a
field only where the type is `#[non_exhaustive]` or has a private field
(`semver-7x.md`), and adding either after 7.0 is itself a break. 7.0 had 64
exhaustive public enums and about a hundred all-`pub` structs, each of which
would have turned the next versioning strategy, OpenTelemetry span kind, MCP
router or boot-error detail into an 8.0.

**A public enum is `#[non_exhaustive]`**, held by clippy's `exhaustive_enums`
in `[workspace.lints.clippy]`. The lint does not know tiers, so the
exceptions are written where they hold, each an
`#[expect(clippy::exhaustive_enums, reason = "…")]`:

- **Closed by a standard or by its own definition, with every caller gaining
  from naming each arm**: Kubernetes' three probes (`ProbeKind`); a delay
  relative or absolute (`Delay`); a proof attempt that ends the proof or not,
  and a proof ending refused or out of budget (`Attempt`, `Unproved`), which
  every driver maps arm by arm; a load by id found, denied or missing
  (`Access`); a reply value or a failure (`ReplyOutcome`), which every
  `#[messages]` expansion matches in the developer's crate. Each says so in
  its rustdoc, where a caller reads it: the `#[expect]`'s reason does not
  render.
- **Tier 2, matched exhaustively by another framework crate**: core's
  `Withheld` (each edge's filing level), config's `PairRefusal` (the HTTP
  server's description of each), http's `VersionLocation` (the OpenAPI
  document's parameter for each), codegen's `Paginate`, `JobDecorator`,
  `JobKey`, `PipeWrapper` and `Posture` (the decorator crates' emitters).
  Tier 2 changes in any release, so `#[non_exhaustive]` buys it nothing,
  while a `_` arm in the matching crate would hide a new variant from the
  framework itself; the exhaustive match across the lockstep crates is the
  check. A tier-2 enum no other crate matches is marked, which costs nothing.
- **What is not API**: the CLI's library (its command line is), and the
  hygiene witness's entity and wire enums, written as a developer writes them.

A framework crate matching another's public enum adds `_` only where the arm
is a real default; otherwise the defining crate grows a method and the caller
reads it, so a new variant is handled where it is declared — the throttler
keys its bucket on `ClientOrigin::ip` rather than on each origin, and the
OpenAPI document reads where a request states its version off
`nest-rs-http`'s tier-2 `version_location`, whose exhaustive match fails the
document's build when a strategy needs a place it does not describe.

**A public struct is `#[non_exhaustive]` or has a private field**, unless no
growth reaches it or the way it is built is how it grows:

- **a configuration, built over its base**: a `#[config]` or a `Default`
  struct nested in one, over `..Default::default()` (`semver-7x.md`), and
  `OpenTelemetryConfig`, over `new` or `from_env`. It is read before the
  container exists, so it is no `#[config]`, but a program builds it the same
  way, and a new field is a minor as a config's is;
- **a newtype over its one value** (`Json`, `Query`, `Path`, `Form`, `Header`,
  `Ctx`, `Valid`, `RawBody`, `Scoped`, `Parameters`, `GuardSpecs`,
  `PipeSpecs`, `TestResponse`): a second field would make it another type,
  and `#[non_exhaustive]` would refuse `Json(value)` in every app;
- **a shape a standard closes**: W3C Trace Context's version-`00`
  `traceparent` (`TraceParent`) and a Server-Timing metric (`Entry`);
- **tier 2**, which changes in any release.

One that a docs page, the demo, another crate or a macro expansion builds
gets its constructor in the same change; an expansion lands in the
developer's crate, where a literal of a `#[non_exhaustive]` struct is
refused, so its constructor is in `__private`.

**A registry entry a program reads has private fields, read through
accessors, and a `const fn` in its crate's `__private`** that only the
expansion calls: `ProcessMethod`'s shape, which `ListenerMethod`,
`ScheduledMethod` and `HealthIndicator` now share, with one read set —
`origin()`, `provider_type_id()` and the facts declared. `provider_type_id()`
is public because a reader of the registry gates on `ReachableProviders` as
the boot does: discovery is module-gated for every reader. An entry carrying
many facts takes them as a tier-2 `*Parts` struct (`HttpRouteParts`,
`ScheduledMethodParts`). **A seam a third party fills by hand has a public
constructor**: `SocialProviderEntry::new::<P, M>` (the provider and the module
gating it as types, so the gate cannot name a non-module),
`GraphqlContextSeed::new`, `CronJobMeta::new` with its `with_*`. An entry only
the framework reads stays whole in `__private`, built by literal (*Refused*).

`AuthenticatedClient` names its `client_id` (the owner's answer of
2026-10-12), and its `actor_id` returns it: a client acting for itself is its
own subject (RFC 9068 §2.2), so the audit of what it does names it. The
demo's refused-scope line therefore names the client through `actor_id`,
which every line of the token request carries, and restates it in no field
of its own.

**Refused:**

- **`clippy::exhaustive_structs`.** Checked on the workspace: it fires on
  every unit marker (`pub struct UsersModule;`, 138 of the 274 sites), which
  no growth reaches. Structs are held by review, and by
  `cargo-semver-checks` once 7.0 is published.
- **Promoting the kernel's module, provider and hook descriptors to a tier-1
  read API.** No program, test outside the kernel, demo or page reads them;
  each decorator's own entry is what a package reads. Promoting them would
  freeze their shape for the whole line with no caller, while tier 2 already
  gives them every room to grow. The same holds for GraphQL's resolver,
  loader and site registrations, the WebSocket namespace entry and the
  soft-delete registration.
- **`#[non_exhaustive]` on every tier-2 enum, as the lint's letter asks.**
  Above: it trades the compiler's check across the framework for nothing.
