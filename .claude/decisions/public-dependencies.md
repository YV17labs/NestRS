# Every dependency is public or private, as Cargo names them

A third-party type in a public signature makes that library's next major a
nestrs major — for a `0.x` library, its next minor. 7.x is meant to last years,
and these libraries cut majors on a shorter clock: opentelemetry went from 0.32
to 0.33 and jsonwebtoken from 10 to 11 while 7.0 was prepared. Until 2026-10-11
one manifest comment covered seven of them, and nothing covered the rest.

**A library is a public dependency when the developer writes its vocabulary in
their own code** — its derives, its attribute grammar, its types in their
signatures — so that a nestrs wrapper would re-implement it or re-export it
whole. A public dependency's major moves only with a nestrs major; its
requirement is `major.minor`, or `=` with the reason at the pin; an app may
name it directly, at a requirement compatible with the framework's. **Every
other library is a private dependency, and the framework wraps it**: no tier-1
signature, field, error variant or re-export names one of its types, and no
tier-1 type implements one of its traits, so its next major is a nestrs minor
or patch. A private dependency's error reaches a public error as a boxed
source — `Arc<dyn Error + Send + Sync>` in an error that is `Clone`,
`Box<dyn Error + Send + Sync>` otherwise, built with
`nest_rs_core::boxed_error` — so its chain reads as before and `source()`
still downcasts to the library's type — through a source the error hands out
itself, since thiserror's `#[source]` on an `Arc` hands out the `Arc`. The
words are Cargo's (RFC 1977 and RFC 3516), and the manifest marks each public
dependency at its pin with `public`; Cargo's own `public = true` is
still unstable in 1.97 (`-Zpublic-dependency`), so the marker is a comment
until it stabilises.

| Library | Decision |
|---|---|
| async-graphql 7 (`=7.2.1`) | **Public**: its derives and `Context` are what a resolver is written in. Its 8.0 carries RUSTSEC-2026-0253's fix, so if 8.0 is released final before 7.0 is published, 7.0 takes it — a public dependency's major is free until the first publish; after that, 8.0 waits for nestrs 8. `async-graphql-poem` leaves the API with poem. |
| rmcp 3 | **Public**: the MCP SDK under the protocol's own organisation; its model types are the MCP schema. |
| sea-orm 2 (`=2.0`), sea-orm-migration 2 | **Public**: `Repo`'s API by design, and `nest_rs::testing::EphemeralDatabase` takes a `MigratorTrait`. |
| validator 0.21 | **Public**: `#[validate(…)]` is the app's. A `0.x`, so its next minor is a nestrs major. |
| schemars 1 | **Public**: `#[derive(JsonSchema)]` and `#[schemars(…)]` are the app's. |
| inventory 0.3 | **Public** for packages (`submit!`) and an app's tests (`iter`); at 0.3 since 2021. |
| redis 1 (`=1.7`) | **Public**: `RedisConnection` implements its `ConnectionLike` so an app sends its own commands, and `redis::RedisError` stays the source of `RedisError`'s variants. |
| rust-embed 8 | **Public**: `#[derive(Embed)] #[folder = "…"]` is the app's. |
| futures-util 0.3 | **Public** while an `#[sse]` route's stream is written in it; the HTTP edge may narrow it to `futures_core::Stream`. |
| async-trait 0.1 | **Public**: every public async trait is implemented through it. Moving one to a native `async fn` is a nestrs major, and `Transport` cannot move while it is used as `Box<dyn Transport>`. |
| tokio 1, tokio-util 0.7, tracing 0.1, serde 1, serde_json 1, anyhow 1, uuid 1, chrono 0.4, http 1, http-body 1, bytes 1 | **Public**: the ecosystem's foundations. `nest-rs-ws` re-exports `serde_json` and `tracing` in tier 1, since a gateway writes both; `nest_rs::http::Body` implements `http_body::Body`. |
| syn 3, proc-macro2 1 | **Public** in `nest-rs-codegen`, whose root takes and returns their types. |
| tokio-rustls 0.26, rustls 0.23 | **Public in `nest-rs-testing` alone**, a dev-dependency: a suite dials a test double through `TestAuthority::acceptor` and `system_connector`. |
| poem | **Private**: 7.0 moves the HTTP edge onto the `http` crate's types and nestrs's own (`http-server.md`). Until that lands, the HTTP edge's public API runs on poem's types — the request, response and error that the filter, interceptor and authentication-strategy traits take (a guard reads `nest_rs::http::Request` through `HttpView` since 2026-10-12, and the in-band seams — GraphQL's operation guard and context seed, MCP's operation guard and tool context, WS's socket context — read it since 2026-10-15; every nestrs extractor reads it too since 2026-10-17, beside the poem twin `#[routes]` still calls), authn's credential readers, the errors and replies implementing its `ResponseError` or `IntoResponse` — and re-exports poem whole at `nest_rs::http::poem` and `nest_rs::ws::poem`, besides `nest_rs::ws::CloseCode`, the prelude's `Json`, `Path` and `Query`, and `nest_rs::testing::TestClient` and its siblings. |
| jsonwebtoken | **Private**: `nest_rs::authn::Algorithm`, which `AuthnConfig` and `JwtOptions` hold, is nestrs's own enum, spelled as RFC 7518 spells `alg`; a crate-private bridge maps it to the library's. |
| opentelemetry (metrics API) | **Private**: an app records through `nest_rs::opentelemetry::Metrics` and its instruments — `Counter`, `UpDownCounter`, `Histogram`, `Gauge`, with `Attribute` and `Describe` — named by the OpenTelemetry Metrics API, and no public signature names an `opentelemetry` type. |
| argon2 | **Private**: its errors are boxed sources of `PasswordError`, shared behind an `Arc` so the error stays `Clone`. |
| object_store | **Private**: its errors are boxed sources of `StorageError`, read with `downcast_ref`, and the backend a store opens is picked by its URL (`storage-port-by-url.md`), so no public signature names an object_store type. |
| tracing-subscriber | **Private**: no public error carries one of its errors, since `nest_rs::core::logging::LogSettings` reads the console's settings into nestrs's own, and no tier-1 type implements one of its traits: the kernel's console writer and formats sit behind core's `__private`, and a binary with no `App` installs the kernel's console through `nest_rs::core::logging::install_console`. The filter syntax `<PREFIX>_LOG` and `RUST_LOG` take is its `EnvFilter`'s, so a major that changes what parses is taken only with that syntax kept, or in a nestrs major. |
| rustls, outside `nest-rs-testing` | **Private**: the process's rustls `CryptoProvider`, which `nest-rs-config` hands to its sibling crates, is reached through its `__private` alone. |
| croner, jiff, jiff-tzdb | **Private**. The cron grammar and the zone names they read are `#[cron]`'s grammar, so a major of either that changes what parses is taken only with that grammar kept, or in a nestrs major. |
| reqwest, oauth2, hyper, h2, sqlx, tokio-tungstenite, and every other dependency | **Private** already: no public signature names them. The record says so, so that it stays true. |

Each wrap above lands before 7.0 is published, and the change that lands it
rewrites its row's "until" clause.

**A vulnerability fixed only in a public dependency's next major ships in the
next nestrs major.** Until then the advisory's `deny.toml` ignore carries a
reason covering what apps can reach — the library is re-exported whole, so an
app can build what the framework never does — and the docs page an app would
reach it from says what not to build. RUSTSEC-2026-0253 is the case today: lru's
`LruCache::pop` is unsound when a key's `Drop` panics, async-graphql 7.2 calls
it from the storage its `dataloader::LruCache` cache factory creates, nestrs
never builds one, and `nest_rs::graphql::async_graphql` lets an app do so; the
dataloaders page says not to.

**Refused:**

- **A second condition — "and it implements the standard the edge speaks".**
  validator, schemars, inventory and rust-embed implement no standard, yet each
  is public by the first; two conditions that disagree with the table do not
  hold it.
- **Wrapping every dependency.** A wrapper over async-graphql's derives or
  sea-orm's query builder is a second framework to maintain, and the developer
  loses the library's own documentation for a copy of it.
- **"Surface" for a public dependency.** The rules already call the runtime
  crate a `*-macros` crate is re-exported from its surface crate; Cargo names
  this pair, and one word holds one concept.

Held by review — a third-party type in a public signature is a public
dependency listed here, or it is wrapped — and by each pin's marker in the root
manifest; once 7.0 is published, `cargo-semver-checks` sees a public
dependency's type change as the break it is.
