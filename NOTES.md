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

## Open code-review findings

None at this revision — the 2026-06-02 post-refactor review landed
(`WsAuthGuard` runtime check, `UsersGraphqlModule` ↔ `OrgsCoreModule`,
strict-resolver-membership boot flag, RAII guard around the
`REACHABLE` thread-local, `LoaderExtension` fail-closed, module-gated
`ContextSeed` + new `forward_principal!(T, Owner)` shape, graceful
`#[use_guards]` resolution in resolvers, friendlier-span rejection of
generic `#[resolver]` impls, `UNAUTHENTICATED` extensions code on
`GraphqlAuthGuard`, resolver-level guards skipped on `#[field]`, and
`ReachableProviders` accepted as global infrastructure). Re-run the
verified baseline above after any change in the same area.

