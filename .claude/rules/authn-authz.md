---
paths:
  - "crates/nest-rs-authn/**/*.rs"
  - "crates/nest-rs-authz/**/*.rs"
  - "crates/nest-rs-social/**/*.rs"
  - "crates/nest-rs-guards/**/*.rs"
  - "demo/crates/features/src/authn/**/*.rs"
  - "demo/crates/features/src/authz/**/*.rs"
  - "demo/crates/features/src/oauth/**/*.rs"
  - "demo/apps/auth/**/*.rs"
  - "**/guard.rs"
  - "**/strategy.rs"
---

# Authn / authz

`nest-rs-authn` answers *who*; `nest-rs-authz` answers *what they may
do*. Compose at the boundary: `#[use_guards(AuthnGuard, AuthzGuard)]`.
The verification alias and the policy live in `demo/crates/features`
(`authn/`, `authz/` + `authz/http/`); apps only mount.

## Absolute rule — only a guard verifies authn/authz

Authentication and authorization are decided in exactly one place: a
`Guard` (`AuthnGuard`/`AuthzGuard`), bound by `#[use_guards(...)]` and —
per operation — by a **visible** `#[authorize(Action, Entity)]` or
`#[public]` that `#[resolver]`/`#[routes]` turns into the gate.

**A parameter type is never a posture.** `Authorized<E, A>`,
`Bind`/`bind`, and the ability-scoped data layer are *enforcement
plumbing* the guard's decision flows into — load the authorized row,
scope the query, mask the response — never the *decision* itself.

Every authn/authz check must therefore be greppable as an
`#[authorize]` / `#[use_guards]` / `#[public]` site. Smuggling the
decision into a parameter type, a service method, or a binding helper is
a **framework defect to remove, not a shortcut**. (This is why a bare
`Authorized<E, A>` parameter is **not** accepted as a standalone posture
— write the `#[authorize]`, then bind the subject in the body.)

## Strategy and principal

**`Strategy`** turns a request into a principal (plain `#[injectable]`,
no macro). **`AuthnGuard<S>`** is generic over it.

`Strategy::authenticate` returns `Result<Self::Principal, AuthError>` —
a pure request → principal mapping that **never issues a transport
response**; a redirect-style flow (OAuth `/authorize`) is a plain
handler, so one trait serves bearer and OAuth alike.

Every `Strategy::Principal` is bound on **`PrincipalIdentity`**
(`actor_id() -> Option<String>`): on success `AuthnGuard` records
`actor_id` onto the request span (pre-declared by the OTel interceptor),
so every downstream event — denials included — is attributable without
per-site threading.

Standard resource-server: `JwtStrategy<C>` ships it; `features::authn`'s
`strategy.rs` writes `type AuthnGuard =
nest_rs_authn::AuthnGuard<JwtStrategy<Claims>>`
once. A guard *alias* binding a strategy is co-located in the strategy's
file, not a separate `guard.rs`.

**`JwtService`** is global infra (factory phase); symmetric secret or
EdDSA key pair — a resource server holds **only the public key** (it
can't mint tokens). So **token issuance is its own app**: `apps/auth`
signs; `apps/api` only verifies. They share `crates/features` and the
DB, **never RPC each other**.

## Authz follows port + adapters

| Folder | Provides |
|---|---|
| `authz/` (root) | `AppAbility`, `AuthzModule` |
| `authz/http/` | `AuthzGuard` (`AbilityGuard<AppAbility>` — **alias in `features`, not in `nest-rs-authz`**), `AuthzHttpModule` |
| `authz/graphql/` | `AppGraphqlGuard` (`GraphqlAbilityBridge<…>`) as `dyn OperationGuard`, `GraphqlAuthnGuard` (`ResolverGuard` marker), `LoaderScope` as `dyn BatchContext`, `AuthzGraphqlModule` + `forward_principal!(Claims)` |
| `authz/ws/` | `WsDataContext` as `dyn SocketContext`, `AuthzWsModule` |
| `authz/mcp/` | `AppMcpGuard` (`nest_rs_mcp::authz::McpAbilityBridge<AuthnGuard, AuthzGuard>`) as `dyn McpOperationGuard`, `AuthzMcpModule` |

**No app-side `authz/` folder** — bridges live with the rest of authz.

## Symmetric pattern across transports

Each feature's `<Feature><Transport>Module` imports its matching
`Authz<Transport>Module` — **and only that** (transports transitively
bring every layer they need).

| Transport | Handler | Guard binding | Module import |
|---|---|---|---|
| HTTP | `#[controller]` | `#[use_guards(AuthnGuard, AuthzGuard)]` on the struct | `[<Feature>Module, AuthzHttpModule]` |
| GraphQL | `#[resolver]` | `#[use_guards(...)]` on the struct + per-op posture `#[authorize(Action, Entity)]` / `#[public]` — **mandatory: no posture ⇒ compile error** | `[<Feature>Module, AuthzGraphqlModule]` |
| WS | `#[gateway]` + `#[messages]` | `#[use_guards(...)]` on the gateway struct (connection-level, on the upgrade request); optional per-event `#[use_guards(...)]` beside a `#[subscribe_message]` | `[<Feature>Module, AuthzWsModule]` |
| MCP | `#[mcp]` tool host | `AppMcpGuard` as `dyn McpOperationGuard` (in-band per operation); **none registered ⇒ the global guard pool, else deny-all** — `AllowAllMcpGuard` is the explicit opt-out for a deliberately public endpoint | `[<Feature>Module, AuthzMcpModule]` |

### Why GraphQL uses a marker but WS binds real guards

HTTP guards run on `&mut Request` before the handler — they *are* the
auth chain.

**GraphQL** runs authn/ability **in-band** per operation, then seeds
`Ability` into per-operation context; the `GraphqlAuthnGuard` **marker**
turns that seeded-context dep into an `#[inject]` the access graph can
validate — omit `AuthzGraphqlModule` ⇒ boot fails naming the missing
guard.

**WS** instead reuses the connection **upgrade** (an HTTP `GET`), so the
gateway binds the real HTTP guards on its struct; they run once at
upgrade and are access-graph-validated the same way — omit
`AuthzWsModule` ⇒ those guards are unreachable ⇒ boot fails. Because the
upgrade's task-locals have unwound by the time a message handler runs,
`WsDataContext` re-seeds executor + ability around each message;
per-message `Guard`s (bound beside a `#[subscribe_message]`, reusing
`Guard::check_ws_message`) add event-level checks when needed. There is
**no** `WsAuthnGuard`/`MessageGuard` marker type — WS reuses the HTTP
`Guard` trait directly.

**MCP mirrors GraphQL, one seam for one seam.** Both are
`EdgePosture::Exempt`, so both gate in-band through their own operation
guard: same authn→authz ordering (`nest_rs_authz::run_ability_chain` — one
function, two error mappings), same global-pool fallback when no bridge is
registered (`FallbackMcpGuard` / `FallbackOperationGuard`), and the *guard's*
`around` installs the ambient ability on both. Two deliberate differences:
`/graphql` carries the `Public` marker so a pooled `AuthnGuard` admits an
anonymous operation through to the resolver gates, and `/mcp` does not — an
unauthenticated tool call is refused; and MCP's *no-pool* tail is deny-all
rather than pass-through, so the fallback can only ever widen what
`use_guards_global` opted into.

## Bound mutations (GraphQL)

A bound mutation receives its subject as an `Authorized<E, A>` parameter,
but **the posture stays explicit**: write `#[authorize(Action, Entity)]`
and load the subject in the body with `bind_required::<Service, A>(ctx,
&id)`, or use the `#[authorize(A, bind = Service)]` form
(container-resolved service) which binds the `Authorized<E, A>` subject
for you.

A bound subject's `Authorized<E, A>` proof is **action-typed**: a `Read`
proof cannot be passed where an `Update` proof is required — a compile
error, not a runtime surprise.

## Public handlers

Omit `#[use_guards(...)]` for that transport and lose the transitive
`Authz<Transport>Module` import — **the app must list it explicitly** if
other handlers need it.
