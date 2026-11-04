# A process that declares no environment runs as production

Settled on 2026-10-11. `openapi-docs-exposure.md` and
`server-timing-exposure.md` open a development surface "in the development and
test profiles only", yet a process that left `<PREFIX>_ENV` unset ran as
development: it read `.env.development` — where the demo turns the GraphQL
playground on — served the documentation, allowed storage's plain HTTP with the
dev container's public pair, and dialled Redis's plaintext loopback. A
forgotten variable is not a visible line.

`Environment::from_env()` now reads an unset, empty or unrecognised
`<PREFIX>_ENV` as `Production`, which is also `Environment::default()`, so an
undeclared process reads production's cascade. A development convenience lives
in a config's `Config::defaults()` behind `Environment::is_deployed` — the one
test, where each crate used to write `Production | Staging` privately — and is
armed only by a process declaring `development` (`dev`) or `test`. The managed
paths declare it: `nestrs run dev` through each workspace's `dev` recipe, the
demo's `db` recipes, and the test harness, which declares `test`.

A process that declares nothing says so once at boot, at `info` on
`nest_rs::config`: a production deployment that omits the variable is correct,
and a `warn` on correct configuration teaches operators to filter the target.
An unrecognised value is read as production and said once on stderr, by the
variable's name and never its value — a misspelt `prodution` no longer loads
the development cascade in production.

ASP.NET Core reads an unset `ASPNETCORE_ENVIRONMENT` as `Production`, and
Laravel's `config/app.php` falls back to `production`; Rails and Express fall
back to development, the convenience this refuses. Security decided: safe by
default, and the opening is one visible line on the process.

**Refused:**

- **Keeping `Development` and gating each convenience on
  `Environment::declared()`.** The cascade's file choice cannot be gated per
  key, so an undeclared process would still read `.env.development`; and every
  new convenience would have to remember the gate.
- **Refusing to boot without `<PREFIX>_ENV`.** An image that sets nothing is a
  correct production deployment; the variable would become one more line every
  deployment must carry, for no security the default does not already give.

Reopened only with a fifth profile, which says on which side of
`is_deployed` it falls.

**2026-10-28 — a development profile arms what no address can say, never an
address.** Two conveniences this decision moved behind `is_deployed` were
addresses: storage's dev-container RustFS with its public pair, and Redis's
plaintext loopback. Neither is what `nestrs new` runs — its `compose.yml`
serves Postgres and Redis on `localhost`, which its `.env.development` and
`.env.test` already name, and no S3 server — so a scaffolded app with storage
dialled a host nothing starts, and Redis's loopback dialled in plaintext what
the dev container serves over TLS alone; SeaORM never had one, since no
framework can know an app's database. One policy now holds for every
resource: no `Config::defaults()` names an endpoint, a URL or a credential,
in any profile, and the profile's `.env` file does — the demo's moved its
services out of the `.env` production reads. What a development profile still
arms is what a URL cannot say: storage's plain HTTP, the documentation,
GraphQL's playground and introspection, Server-Timing, an artifact's emit, a
seed. The console's format joins them: text under a declared development or
test profile, JSON otherwise, where it followed the cargo build profile and
gave `development` a second meaning — a release build under it wrote JSON, a
debug build in production text. The kernel reads the profile through the
config crate the binary links, when it builds the console.

**Refused:**

- **Every resource defaulting to the scaffold's local services.** SeaORM
  cannot — the database's name and credentials are the app's — and a hostname
  compiled into the framework dials what the developer's compose file may not
  run.
- **`#[nest_rs::main]` handing the profile to the kernel.** A test process and
  a binary calling `load_cascade` run no `main` macro, and would log JSON while
  declaring `test` or `development`; the kernel asks the linked config crate
  instead, as late as it builds the console.

**2026-10-31 — every app that reads a config says it.** The line lived in
`ConfigModule::for_root()`'s collect, an import the demo's roots wrote first and
`nestrs new`'s apps never wrote, so a scaffolded app running as production in
silence was the common case. The first config a boot loads — through
`ConfigModule::provide_feature` or `provide_root`, which every module owning a
config and every injected config route through — now registers `Environment`
and files the line, once per boot; `ConfigModule::for_root()` is gone
(`.claude/decisions/bare-import-reads-for-root-pins.md`). The line keeps its
level and its words.

**2026-11-04 — a config has one base, and a development opening resolves where
it is used.** Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)" to
the identity audit's second pass, its cfg-2). This entry replaces the sentence
above placing a development convenience in `Config::defaults()`, and the
2026-10-28 entry's "no `Config::defaults()` names an endpoint", which holds of
`Default`. That hook was a second base: `GraphqlConfig { max_depth: Some(12),
..Default::default() }`, the form the configuration page teaches, turned the
playground, introspection and the SDL emit off under development with no line
to say so, and the base that kept them took a trait import to reach. The console
kept the opposite rule, its `Default` reading the profile, and `LogFormat`'s own
default was `Text`, the development value.

A profile-gated setting is now the value `nest_rs::config::Surface`:
`ByProfile`, `Default`'s, `Open` and `Closed`, written in code or read from a
boolean. The consumer resolves it where it uses it, the framework's own at the
boot, `is_open()` or, for an emit, `writes()`. A deployed or undeclared process resolves `ByProfile` closed, so
production is unchanged and a deployment's opening is the same visible line; what
moved is development, where a pin over `..Default::default()` now keeps its
surfaces. `Config::defaults()` and `#[config(defaults = …)]` are gone, and
`Default` is the one base a bare import, a named instance and a discovered
provider read. `LogFormat` takes the same shape, `ByProfile`, `Text` and `Json`,
resolved where the console is built, so no `Default` reads the process.
`is_deployed` stays the one test, read in one place.

**Refused:**

- **A `Default` reading the profile, as the console's did.** A value then carries
  the profile of the moment it was built, a config's `Default` differs by process,
  and the overlay still cannot tell an opening written in code from one inherited.
- **A profile rule in the overlay.** It would rewrite a deliberate pin, which is
  why the hook existed; a written `Open` or `Closed` is never touched.
- **`Option<bool>` for the three states.** `None` is off and nothing else
  (`config-overlay-derived.md`).
- **`From<bool>` for `Surface`, or a `bool` field beside it.** A second spelling
  of `Open`, in a set where the point is to read which of the three was written.
- **A second enum for an emit.** It has the same three states and differs only
  in which profiles `ByProfile` opens; the consumer says so by the reader it
  calls, where a parallel type would spread one fact over two names.
