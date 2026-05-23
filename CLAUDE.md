# CLAUDE.md — nestrs

For an LLM picking up this repository. The codebase tells you what *is*; this
file tells you what was decided and what must be respected.

This file is committed to a public repository. Do not introduce machine-local
paths, references to private memory, or links to personal resources.

## What this project is

nestrs is an opinionated Rust framework whose central bet is procedural macros.
Crates under `crates/` provide the building blocks (IoC container, module
trait, the decorator-style macros). Binaries under `apps/<name>/` are real
applications that consume those crates.

NestJS inspired the surface; it is no longer the reference. Describe features
by what they do, not by reflex-pointing at a Nest equivalent.

## The rule that shapes every change

**Reach for the macros first.** `#[injectable]`, `#[module]`, `#[controller]`,
`#[routes]`, and the per-verb attributes are how application code stays
declarative. When you wire a new service, a feature module, or an HTTP
endpoint, use them. When a pattern recurs and no macro covers it, **write a
new decorator macro** rather than duplicate the boilerplate by hand.

Because a `proc-macro` crate can export only macros, each decorator lives in a
companion `*-macros` crate re-exported by its home crate: the
surface-agnostic `#[injectable]`/`#[module]` in `nestrs-macros` (re-exported
by `nestrs-core`); a surface's decorator in that surface's `*-macros` crate
(`#[controller]`/`#[routes]`/`#[interceptor]` in `nestrs-http-macros`,
`#[resolver]` (with method-level `#[query]`/`#[mutation]`) in
`nestrs-graphql-macros`, `#[mcp]` in
`nestrs-mcp-macros`), re-exported by the surface so apps import it from there
(`nestrs_http::controller`). Shared token-building helpers go in
`nestrs-macro-support`; a `*-macros` crate must not depend on its surface
crate (it emits absolute-path tokens resolved at the call site), so there is
no cycle. The macros are the leverage the project pays to maintain; spending
them aggressively is the point.

## Dependency injection is internal

The Rust DI ecosystem was surveyed; none of the active candidates met our
maintenance bar. The container in `crates/nestrs-core` is ours and stays ours.
**Do not propose adopting an external DI crate.** If ergonomics fall short,
extend ours.

## Discovery is struct-level by default

Anything a module wires up — providers, controllers, interceptors, future
cron jobs, event handlers, MCP tools, … — implements `Discoverable` and is
declared in a single flat `#[module(providers = [...])]` list. The container
indexes attached metadata by type; transports and applicative scanners read
it via `DiscoveryService::meta::<MetaT>()`. The `#[module]` macro itself is
generic — it knows nothing about HTTP, MCP, or any specific surface.

**Default to one struct per concern.** A cron job is a struct, an event
handler is a struct, an MCP tool is a struct. Each carries its own
decorator macro (`#[cron_job]`, `#[event_handler]`, `#[mcp_tool]`, …) that
emits the single `impl Discoverable for Self` — no conflict, no central
registry to update, third-party crates extend the system without touching
`nestrs-macros`. **HTTP and GraphQL are the exceptions**: `#[routes]`
orchestrates verb attributes (`#[get]`, `#[post]`, …) on a controller's impl
block, and `#[resolver]` orchestrates `#[query]`/`#[mutation]`/`#[field]` on a
resolver's impl block — because regrouping endpoints (or splitting one
resolver's queries, mutations, and field resolvers) into a struct each would be
absurd. async-graphql forces the split: `#[Object]` makes an entire impl one
root, so method-level kind is the only way to keep a feature's resolver in one
struct. `#[field]` (the field-resolver verb, NestJS's `@ResolveField`) is the
third member of that set, justified by the same logic: a feature's root
queries and the computed/related fields of its types belong in one resolver,
exactly as a NestJS `@Resolver(() => T)` class holds both. Its parameters mirror
NestJS's `@Parent`/`@Args`/`@Loader`: the first, `parent: &T`, is the resolved
object; owned parameters are GraphQL arguments; `&`-reference parameters (a
service, a `DataLoader<…>`) are injected from the container — unambiguous since a
`&T` can never be a GraphQL `InputType`. The macro emits one
`#[ComplexObject] impl T` per parent type, building the resolver from the
container in the GraphQL context. Because async-graphql allows a single
`ComplexObject` per type, a type's fields are owned by one resolver, and `T`
must derive `#[graphql(complex)]`. Any *further* method-level decoration needs
a strong justification and a written design note.

Batch field-resolver fetches with `#[dataloader]` to avoid N+1s. It is an
impl-block decorator on the **data layer** (the service — where the future ORM
query will live), not a loose loader struct: each method
`async fn name(&self, keys: &[K]) -> HashMap<K, V>` (optionally `Result<…, E>`)
generates a hidden `Loader` named `<Owner><Name>` (e.g. `UsersServiceByName`)
wrapping `Arc<Owner>` and delegating to the method — no `#[module(providers =
[...])]` entry. The loaders are **request-scoped, like NestJS**: a schema
extension installed by `GraphqlModule` rebuilds every discovered loader from the
fully assembled container at the start of each request and seeds it into the
GraphQL context, where a `#[field]` reads it back as `&DataLoader<UsersServiceByName>`.
Concurrent `load_one` calls within one request collapse into a single
`Loader::load` (killing the N+1); the per-request instance means no leakage
across requests and lets a loader observe per-request state. Because the loader
is built when a request arrives — not at module registration — `GraphqlModule`'s
import order relative to the data modules it loads is irrelevant, preserving the
project's order-independence guarantee. (A `#[field]` distinguishes the two
injection scopes by type: a `&DataLoader<…>` comes from the request context, any
other `&Service` from the container.)

GraphQL composition is **discovered, not listed**. Each `#[resolver]` impl
submits its generated query/mutation objects to a link-time `inventory`
registry; the schema's roots (`DiscoveredQuery`/`DiscoveredMutation`) merge
their fields from that registry at boot, so there is no central `queries =
[...]` list. This works *despite* async-graphql's static `Schema<Q, M, S>`:
the roots are concrete types whose `create_type_info` reads the registry (via
`Registry::create_fake_output_type`) and whose `is_empty` reports emptiness at
runtime. Import `GraphqlModule` to self-mount the schema at `/graphql`. The
cost is a reliance on async-graphql's public-but-internal `registry` API —
guarded by compile errors and tests when it shifts. Field resolvers are the
exception to this runtime merge: `#[field]` lowers to async-graphql's native
`#[ComplexObject]`, so its fields attach to their type statically — no
registry, no roots.

## Naming rules — strict

- Applications live under `apps/<name>/`. Not `examples/`, not `services/`.
  The first was rejected because these are real applications, not samples; the
  second because the project is not microservices-oriented.
- File names follow Rust snake_case: `service.rs`, `controller.rs`,
  `resolver.rs`, `module.rs`, `dto.rs`, `entity.rs`. Do not invent dotted
  variants — they are not valid Rust module names.
- A file exists only if it has real content. No placeholders for symmetry.
- `lib.rs` is the crate's index, not its implementation. Keep it to the
  crate-level `//!` doc, `mod` declarations, and `pub use` re-exports.
  Logic belongs in topical modules. Exception: very small crates (~100
  lines total) may inline everything.

## Dependency bar

Every new third-party crate must have a published release within the last
~12 months. If a candidate fails this bar, flag it explicitly in the proposal.
Do not add a stale dependency silently.

## "Done" means verified live

For HTTP or GraphQL changes, `cargo test --workspace` is necessary but not
sufficient. Start the binary (`cargo run --bin <app>` in the background),
`curl` the affected endpoints, then kill the server before returning control.
Routing and wiring bugs do not surface in unit tests.

A GraphQL app commits its schema as SDL (`apps/<app>/schema.graphql`) so the API
surface is reviewable in diffs. After changing resolvers, regenerate it with
`just graphql-schema <app>` (default `api`); `just graphql-schema-check <app>`
regenerates in memory and fails if the committed file drifted — wire it into CI.
`nestrs_graphql::schema_sdl` renders it with sorted types/fields/arguments so it
is deterministic across builds. The schema is composed from the resolvers
*linked into a binary*, so it can only be rendered from inside the app: each
GraphQL app's binary exposes a `schema` subcommand (`<app> schema [--check]`,
checked before the server boots in `main`) that calls
`nestrs_graphql_cli::run::<AppModule>(…)` — the shared emit/drift-check logic,
built on `App::context` (container, no transport). The path is the app's own via
`CARGO_MANIFEST_DIR`, so it adapts to any app name. `nestrs-graphql-cli` is also
where federation-aware schema commands will land if/when federation does.

## Engineering posture

- No premature abstraction. Extract after a pattern appears twice, not before.
- Strict typing. Enums over string-typed states. Parse at the edge using
  established crates (`validator` for declarative input checks, `uuid` for
  UUID v7 IDs) rather than hand-rolling newtypes for every format-validated
  string. Reserve newtypes for values whose *meaning* — not just format —
  needs the type system's help. Avoid `Box<dyn Any>` and `serde_json::Value`
  passthrough unless the data is genuinely unstructured.
- Errors at boundaries: `thiserror` in libraries, `anyhow::Result` at the
  application entry. No `unwrap()` on production paths.
- Doc comments only where the *why* is non-obvious. Never paraphrase the
  type or function name.
- Macro-generated code uses absolute paths (`::nestrs_core::*`, `::poem::*`,
  `::std::sync::Arc`). Never rely on what is in scope at the call site.

## Hard "no" list

- No external DI library.
- No renaming of `apps/`.
- No feature flags for capabilities that do not yet exist.
- No backwards-compatibility shims (no public API to preserve yet).
- No mocking the database in integration tests when persistence lands — use
  `testcontainers` against real Postgres.
- No splitting the workspace into microservices "for scalability". The scope
  is multiple applications sharing libraries.

## Workflow

State the plan in one or two sentences before invoking tools. Batch
independent tool calls in parallel. Run `cargo test --workspace` after
meaningful changes; verify live as above for routing changes. Kill any
background server before returning control. Report what changed and what was
verified — no paragraph-long summary.

## What this file deliberately does not contain

The crate layout, the dependency versions, the macro signatures, the test
counts, the file tree. The code is authoritative on those — read it. This
file only states what the code cannot.
