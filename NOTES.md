# NOTES.md — nestrs operational state

This file holds the **current** weather of the project — what works, what is
known-broken, what is queued. Unlike `CLAUDE.md` (durable rules), this file
moves: it is rewritten as items resolve. If you read it after a long pause,
treat every concrete claim (test counts, commands, file paths) as needing
verification before you act on it.

Update conventions:
- A resolved item is removed (not crossed out) — the git history is the
  changelog.
- Add a date in `YYYY-MM-DD` on an item that has been open for more than one
  cycle, so a future reader can judge whether the gap is fresh or stale.

## Verified baseline

Re-run after any authz, HTTP, or feature-data change:

```
NESTRS_DATABASE__URL=postgres://nestrs:nestrs@postgres:5432/nestrs cargo test -p api --test e2e
cargo test -p auth --test e2e
cargo test -p features
```

At time of writing: `api/e2e` = 14 tests, `auth/e2e` = 10 tests, `features`
covers authn aliases + authz policy + oauth grants. Outside the
devcontainer (no Postgres), expect e2e + DB-backed `features` tests to
fail with `NESTRS_DATABASE__URL must point at a reachable Postgres`; this
is environment, not regression.

## Open work — known gaps, not blockers for unrelated tasks

| Area | What |
|------|------|
| `nestrs-authz` (`http` feature) | Integration tests for `shape.rs` cover wire→mask→retain; expand to assert no `password_hash` leak in a domain-like scenario |
| `#[expose]` | Extend `wire.rs` default emission beyond `String`/`Option`/`bool`/numerics/`Uuid`/`DateTime*` — `Decimal`, custom enums, and other sea_orm types currently need a hand-written `impl WireModelDefaults` |
| `features::users` | DB-backed tests for `users` `#[dataloader]` batches |
| `features::oauth` | Unit tests for `OAuthFlow::resolve_caller` — needs `OAuth2Client` to become a trait (or a test-double) so the HTTP callout can be stubbed; `authenticate_client` likewise blocked on `OAuthFlow::new` requiring all four deps |
| Live check | Some PRs require `cargo run -p api` + `curl` + kill — see CONTRIBUTING |

## Feature exemplars — what to copy from when adding code

The repo holds a few canonical features; copy their shape before inventing
your own. When the exemplar no longer fits, fix it (and update this list).

- **`crates/features/src/users/`** — the reference feature, fully Layout D.
  - `core/`: entity, service, dto, error, module (`UsersCoreModule`). The
    service holds `CrudService`, `#[dataloader]`, credentials helpers,
    `#[hooks]` in one `service.rs`. Pure `UserError` (`Clone` for
    DataLoader). `UsersService::new(db)` is public for tests.
  - `http/`: controller + `error.rs` (`impl ResponseError` for `UserError`
    — orphan-safe inside the same crate) + module (`UsersHttpModule`,
    `imports = [UsersCoreModule, AuthzHttpModule]`). Custom `POST /users`
    returns `Json<User>` (DTO), not `Model`.
  - `graphql/`: single `UsersResolver` with `#[field]` (relations) and
    `#[query]`/`#[mutation]` (roots) merged + module
    (`UsersGraphqlModule`, `imports = [UsersCoreModule, AuthzGraphqlModule]`).
    The resolver impl block binds `#[use_guards(GraphqlAuthGuard)]`.
  - `ws/`: gateway + module (`UsersWsModule`,
    `imports = [UsersCoreModule, AuthzWsModule]`). The gateway binds
    `#[use_guards(AuthGuard, AppAbilityGuard)]` (connection) and each
    `#[subscribe_message]` binds `#[use_guards(WsAuthGuard)]`
    (access-graph marker).
- **`crates/features/src/oauth/`** — `core/` holds `TokenIssuer` (signs
  claims into an `AccessToken`) and `OAuthFlow` (Authorization Code
  exchange, client-credentials validation in constant time). `strategy.rs`
  is a **thin HTTP adapter** over `OAuthFlow` (Poem request/response only —
  every grant decision lives in the service). `core/error.rs` +
  `http/error.rs` at the boundary. `http/` adds the `/token`,
  `/authorize`, `/callback`, `/login` controller and `OAuthHttpModule`.
  Grant logic tests in `features/tests/oauth/`.
- **`crates/features/src/authz/`** — port + adapters for policy: `core/`
  has `AppAbility`, `http/` the `AppAbilityGuard`, `graphql/` the
  `AppGraphqlGuard` bridge + `LoaderScope` provider + `forward_principal!`,
  `ws/` the `WsDataContext` provider. App imports per transport served.
- **`crates/nestrs-authn/`** — the strict-mirror test layout reference:
  one `tests/authn.rs` entry, `tests/<role>/mod.rs` mirrors `src/<role>/`.
- **`apps/api/`** — the most complete app (REST + GraphQL + WS + DB + authz).
- **`apps/chat/`** — the pure real-time exemplar (WS-only, no DB).

## Open code-review findings (2026-06-02)

From the post-refactor multi-angle review of the Layout D + module-gated
discovery + symmetric three-transport-authz changes. Ranked by severity;
attack top-down.

### Security / correctness

1. **`WsAuthGuard` fails OPEN by design.** `can_activate` returns `Ok(())`
   unconditionally (`crates/features/src/authz/ws/guard.rs:30`), and the
   paired `WsDataContext::capture` installs `Executor::Pool` even when no
   `Ability` was captured. Combined with the framework rule "no ability ⇒
   `condition_for => TRUE` => unscoped read", a mis-wired gateway (e.g.
   future namespaced gateway that drops connection-level `AuthGuard`)
   serves cross-tenant data. **Fix:** make `can_activate` assert the
   connection installed an `Ability` snapshot before returning Ok.

2. **`UsersGraphqlModule` does not import `OrgsCoreModule`.** The
   `User.org` field uses `&DataLoader<OrgsServiceById>`, whose loader
   resolves `OrgsService`. The macro deliberately excludes `DataLoader`
   types from `injected_deps`
   (`crates/nestrs-graphql-macros/src/resolver.rs:483-489`), so the
   access graph cannot see the dependency. A users-only app would boot
   cleanly and the first `User.org` query would panic at
   `data_unchecked`. Today masked because `apps/api` also imports
   `OrgsHttpModule`. **Fix:** add `OrgsCoreModule` to
   `UsersGraphqlModule.imports`, mirror in any future cross-feature
   `#[field]` dataloader.

### Boot/discovery

3. **Removed `validate_resolver_membership` ⇒ silent boot.** A
   linked-but-unreachable resolver now warns via `tracing::warn` instead
   of failing the boot (`crates/nestrs-core/src/app.rs:51`). In
   `RUST_LOG=info` prod, a forgotten `OrgsGraphqlModule` import yields
   `Unknown field` at request time with no CI gate. **Fix:** either keep
   warn (current) and add a smoke check that asserts schema field
   counts, or make it an opt-out error (`strict_membership` builder
   flag).

4. **`REACHABLE` thread-local not panic-safe.** Cleared after
   `Schema::build` via a non-RAII line
   (`crates/nestrs-graphql/src/resolver.rs:252`). A panic between set
   and clear leaks the previous app's set on the thread, breaking
   multi-app test isolation. **Fix:** RAII drop guard.

5. **`LoaderExtension::prepare_request` falls back to "seed all" when
   `ReachableProviders` is missing** (`loader.rs:117`). Hand-rolled
   container construction bypasses the gate; loaders whose owner isn't
   registered panic at request time. **Fix:** default to "skip all" so
   absence of the gate fails closed, or assert at construction.

6. **`forward_principal!(Claims)` moved into library crate.** Its
   `inventory::submit!` is now link-time global to every consumer of
   `features` (`crates/features/src/authz/graphql/module.rs:21`). Today
   dormant in `apps/auth`. A future second GraphQL app with a different
   principal type would have both forwarders fire. **Fix:** module-gate
   the `ContextSeed` mechanism the same way `ResolverRegistration` was
   gated, or move the macro call back to per-app composition.

### Macro / hot-path

7. **`use_guards` resolves with `.expect(...)`** in
   `nestrs-graphql-macros/src/resolver.rs:180` and `nestrs-ws-macros/
   src/messages.rs:380-388`. The access graph is supposed to make this
   unreachable, but anything that slips past panics on a Tokio worker —
   violates the "no unwrap in hot paths" rule.

8. **`root_object` emits `TypeId::of::<#self_ty>()` unconditionally**
   for the resolver impl (`resolver.rs:511`), unlike `resolver_struct`
   which guards generics. A `#[resolver] impl<'a> Foo<'a>` fails to
   compile with a confusing deep-in-macro error. **Fix:** reject
   non-`'static` self types in `impl_self_ident` with a friendly span.

### Polish

9. **`GraphqlAuthGuard` error has no GraphQL `extensions.code`**
   (`crates/features/src/authz/graphql/guard.rs:34`). Inconsistent with
   `authorize()`/`bind()` which use `FORBIDDEN`. **Fix:** use
   `ErrorExtensions` with `code = "UNAUTHENTICATED"` and trim the
   leaking impl message.

10. **Resolver-level `#[use_guards]` runs on every `#[field]`**
    (`resolver.rs:461`). `GraphqlAuthGuard::check` probes
    `ctx.data_opt` per row in list results. Functionally OK but
    redundant overhead. **Fix:** either gate field methods separately or
    document the duplication.

11. **`ReachableProviders` seeded *after* `global = builder.provider_ids()`**
    (`app.rs:42`). A future `#[inject] Arc<ReachableProviders>` would
    fail the access graph despite the value being live. Latent footgun.

12. **Stale doc** in `crates/nestrs-authz/src/graphql/bridge.rs:16-18`
    references `features::authz::AuthzModule` (deleted) and
    `ApiGraphqlGuard` (renamed). Also
    `crates/features/tests/authz/mod.rs:2` mentions
    `apps/api/src/authz/module.rs` (deleted).

