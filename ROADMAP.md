# Roadmap

NestRS is in **alpha** — the foundations are in place and the API still shifts.
This is a *direction, not a dated commitment*; priorities move with what the
community needs.

Want to shape it? Open a
[Discussion](https://github.com/maltyxx/nestrs/discussions) or pick up a
[`good first issue`](https://github.com/maltyxx/nestrs/labels/good%20first%20issue).
The authoritative record of *what was decided and why* is
[CLAUDE.md](CLAUDE.md); this file tracks *what's next*.

## Now — stabilising the alpha

- Settle the public API of the core crates so early adopters stop chasing
  breaking changes.
- **Published benchmarks** — replace the "native-Rust-vs-Node" framing with
  reproducible throughput, memory, and cold-start numbers.
- Fill in crate-level docs and grow the `apps/` examples.
- **First-class testing utilities** — an in-process testing module that boots an
  app and fires HTTP / GraphQL requests inside `cargo test`, with provider
  overrides for mocking dependencies (the `Test.createTestingModule` analog).
  Today wiring is verified by running the binary; this brings it into the test
  suite.
- **Richer boot diagnostics** — when the DI graph can't be satisfied, name the
  offending provider and the missing dependency, and surface cycles at boot
  rather than at first use.

## Next — the documented gaps

These are known, deliberate omissions called out in the code today:

- **OpenAPI** — query-parameter schemas, real path-parameter *types* (emitted as
  `string` for now), security schemes, and a committed `openapi.json` snapshot
  written on boot (mirroring how the GraphQL SDL is committed).
- **Guards** — declarative per-handler metadata a guard can read to vary
  behaviour: the `@Roles` / `Reflector` analog.
- **Scheduling** — cron expressions. Today `#[cron_job]` takes fixed intervals
  only (`ms` / `s` / `m` / `h`), pending a parser that clears the dependency bar.
- **Dependency-injection scopes** — request- and transient-scoped providers. The
  container is singleton-only today; per-request state is carried ad hoc through
  extractors and request-scoped DataLoaders.
- **GraphQL authorization** — extend the CASL-style `nestrs-authz` to resolvers;
  it binds to HTTP routes only today.
- **Events** — an event bus and an `#[event_handler]` decorator, the
  discovered-concern analog of `@nestjs/event-emitter`.
- **`nestrs-resource`** — relations, enums, and pagination types for the
  entity-to-API resource macro, which is experimental today.
- **HTTP cross-cutting** — CORS, global exception filters (the `@Catch` analog),
  and API versioning.
- **Config** — a validated, injectable config service, plus optional
  dependencies (`Option<Arc<T>>`, the `@Optional` analog).

## Later — exploring

- GraphQL **federation**, and the dedicated schema tooling it would reintroduce.
- More transports and surfaces as the discovery model proves out.

## Not on the roadmap

By design — see the *Hard "no" list* in [CLAUDE.md](CLAUDE.md):

- No external dependency-injection library — the container is ours.
- No splitting the workspace into microservices "for scalability".
- No backwards-compatibility shims while the API is pre-1.0.
