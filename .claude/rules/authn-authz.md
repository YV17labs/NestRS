---
paths:
  - "crates/nest-rs-authn/**/*.rs"
  - "crates/nest-rs-authz/**/*.rs"
  - "crates/nest-rs-oauth-client/**/*.rs"
  - "crates/nest-rs-oauth-resource/**/*.rs"
  - "crates/nest-rs-oauth-server/**/*.rs"
  - "crates/nest-rs-social/**/*.rs"
  - "crates/nest-rs-guards/**/*.rs"
  - "demo/crates/features/src/app_authn/**/*.rs"
  - "demo/crates/features/src/app_authz/**/*.rs"
  - "demo/crates/features/src/app_oauth/**/*.rs"
  - "demo/apps/auth/**/*.rs"
  - "**/guard.rs"
  - "**/strategy.rs"
---

# Authn / authz

`nest-rs-authn` answers *who*; `nest-rs-authz` answers *what they may
do*. Compose at the boundary: `#[use_guards(AppAuthnGuard, AppAuthzGuard)]`.
The verification alias and the policy live in `demo/crates/features`
(`app_authn/`, `app_authz/` + `app_authz/http/`); apps only mount.

## Absolute rule — only a guard verifies authn/authz

Authentication and authorization are decided in exactly one place: a
`Guard` (`AppAuthnGuard`/`AppAuthzGuard`), bound by `#[use_guards(...)]` and —
per operation — by a **visible** `#[authorize(Action, Entity)]` or
`#[public]` that `#[operations]`/`#[routes]` turns into the gate.

**A parameter type is never a posture.** `Authorized<A, E>`,
`Bind`/`bind`, and the ability-scoped data layer are *enforcement
plumbing* the guard's decision flows into — load the authorized row,
scope the query, mask the response — never the *decision* itself.

Every authn/authz check must therefore be greppable as an
`#[authorize]` / `#[use_guards]` / `#[public]` site. Smuggling the
decision into a parameter type, a service method, or a binding helper is
a **framework defect to remove, not a shortcut**. (This is why a bare
`Authorized<A, E>` parameter is **not** accepted as a standalone posture
— write the `#[authorize]`, then bind the subject in the body.)

**`#[public]` selects which half of the policy runs.** `AbilityGuard`
builds an anonymous caller's rules from `AbilityFactory::define_visitor`
(default: grants nothing — a route opened with `#[public]` still exposes
nothing until a rule is written); the same route reached *with* a valid
token takes `define`. The decision still happens in the guard, so the
three greppable sites stand — but what a `#[public]` route exposes is no
longer readable from the route alone. **Review the pair**: the marker,
and what `define_visitor` grants for that entity. An edit to
`define_visitor` is a route-exposure change and carries that weight.

**Public reads: `#[public]` + a hand-written `Authorize<A, E>` — the one
sanctioned exception** to "never written by hand" below. A DB-backed
route anyone may read cannot use `#[authorize]`: it and `#[public]`
declare opposite postures, so writing both is a compile error. The shaper
parameter is spelled out instead — still enforcement plumbing, never the
posture — and without it the route installs no ambient ability, so `Repo`
denies every row and the caller reads an empty `200`. Nowhere else is
`Authorize` hand-written; anywhere else it is an `#[authorize]` that was
bypassed. Exemplars: `nest-rs-seaorm`'s `public_visitor` e2e, and the
[Public reads](https://nestrs.dev/security/authorization/public-reads/)
page.

**Scopes gate a rule, they are not a second decision site.**
`.requires_scope("posts:read")` on an `AbilityBuilder` rule withholds
that rule when the caller's credential does not carry the scope — the
rule is *not added*, so the gate, the query pre-filter and the mask all
refuse together exactly as for a rule nobody wrote. The decision stays
in the guard; the scope only says what the credential must carry for the
rule to exist. Scopes **narrow, never widen**: they cannot grant what
the role denies.

The credential reports what it carries through
`PrincipalIdentity::scopes()` — `None` (the default) means *not
scope-aware*, so scoped rules apply in full; `Some(&[])` means *an OAuth
credential delegated nothing*, so they are all withheld. A bearer-token
claims type returns `Some`; conflating the two is the fail-open reading.
`AppAuthnGuard` publishes it as `nest_rs_guards::GrantedScopes`, which is
how authn tells authz without either crate depending on the other.

A refusal for want of a scope is `Denial::InsufficientScope`, not
`Forbidden` — the client can act on the first and not the second. It
reaches the edge as `RequiredScopes` on the response, where
`ResourceChallenge` renders the RFC 6750 `insufficient_scope` challenge
for HTTP/WS/MCP, and as an `INSUFFICIENT_SCOPE` + `requiredScopes` error
frame on GraphQL. **Convert a denial to a poem `Err` with
`denial_to_http_error`, never `Error::from_response(denial_to_http_response(..))`**
— poem's `into_response` overwrites the response's extensions, silently
dropping the evidence. Every scope a rule requires must appear in
`NESTRS_OAUTH_DISCOVERY__SCOPES_SUPPORTED`, or the client is told to request what
discovery never names (reported at `warn`, `reason="scope_not_advertised"`).

**Non-CRUD routes: a capability-only guard IS the sanctioned pattern.**
A route whose response is not an entity row (a presigned URL, a computed
report) gates through a custom `Guard` that checks the ability
imperatively (`ability.can_class(...)`), bound via `#[use_guards(...)]`
— writing `#[authorize(...)]` there would arm response masking against a
body that is no wire model and fail closed at 500. The check stays
greppable (the `#[use_guards]` site) and the guard logs its denial at
`warn`. Exemplar: `audio`'s `TranscodeGuard`.

`Authorize<A, S>` is the extractor `#[authorize]` **desugars to** (the
same one `#[crud]` emits) — enforcement plumbing, never written by hand
outside the `#[public]` exception above. `#[routes]` arms the shaper **by
type**: it hands every parameter type to `nest_rs_http::ShaperProbe` and the
compiler answers, so an alias, a re-export or any qualification arms
identically. The decorator still matters, because it is what makes the posture
greppable — the arm being alias-proof is a fail-secure property, not a licence
to hand-write the parameter.

## Strategy and principal

**`Strategy`** turns a request into a principal (plain `#[injectable]`,
no macro). **`AppAuthnGuard<S>`** is generic over it.

`Strategy::authenticate` returns `Result<Self::Principal, AuthError>` —
a pure request → principal mapping that **never issues a transport
response**; a redirect-style flow (OAuth `/authorize`) is a plain
handler, so one trait serves bearer and OAuth alike.

Every `Strategy::Principal` is bound on **`PrincipalIdentity`**
(`actor_id() -> Option<String>`): on success `AppAuthnGuard` records
`actor_id` onto the request span (pre-declared by `operation_span!`, which
is the only place the canonical field vocabulary is written),
so every downstream event — denials included — is attributable without
per-site threading.

Standard resource server: `JwtStrategy<C>` ships it; `features::app_authn`'s
`strategy.rs` writes `type AppAuthnGuard =
nest_rs_authn::AppAuthnGuard<JwtStrategy<Claims>>`
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
| `app_authz/` (root) | `AppAbility`, `AppAuthzModule` |
| `app_authz/http/` | `AppAuthzGuard` (`AbilityGuard<AppAbility>` — **alias in `features`, not in `nest-rs-authz`**), `AppAuthzHttpModule` |
| `app_authz/graphql/` | `AppGraphqlGuard` (`GraphqlAbilityBridge<…>`) as `dyn OperationGuard`, `LoaderScope` as `dyn BatchContext`, `AppAuthzGraphqlModule` + `forward_principal!(Claims)` |
| `app_authz/ws/` | `WsDataContext` as `dyn SocketContext`, `AppAuthzWsModule` |
| `app_authz/mcp/` | `AppMcpGuard` (`nest_rs_authz::mcp::McpAbilityBridge<AppAuthnGuard, AppAuthzGuard>`) as `dyn McpOperationGuard`, `AppAuthzMcpModule` |

**No app-side `app_authz/` folder** — bridges live with the rest of authz.

## Symmetric pattern across transports

Each feature's `<Feature><Transport>Module` imports its matching
`Authz<Transport>Module` — **and only that** (transports transitively
bring every layer they need).

| Transport | Handler | Guard binding | Module import |
|---|---|---|---|
| HTTP | `#[controller]` | `#[use_guards(AppAuthnGuard, AppAuthzGuard)]` on the struct + per-route posture `#[authorize(Action, Entity)]` / `#[public]` — optional (a non-CRUD route gates through a capability-only guard instead) | `[<Feature>Module, AppAuthzHttpModule]` |
| GraphQL | `#[resolver]` + `#[operations]` | `#[use_guards(...)]` on the struct + per-op posture `#[authorize(Action, Entity)]` / `#[public]` — **mandatory: no posture ⇒ compile error** | `[<Feature>Module, AppAuthzGraphqlModule]` |
| WS | `#[gateway]` + `#[messages]` | `#[use_guards(...)]` on the gateway struct (connection-level, on the upgrade request); optional per-event `#[use_guards(...)]` beside a `#[subscribe_message]`, plus a per-message posture `#[authorize(Action, Entity)]` / `#[public]` — **mandatory: no posture ⇒ compile error**, as on GraphQL and MCP | `[<Feature>Module, AppAuthzWsModule]` |
| MCP | `#[mcp]` host | `AppMcpGuard` as `dyn McpOperationGuard` (in-band per operation); **none registered ⇒ the global guard pool, else deny-all** — `AllowAllMcpGuard` is the explicit opt-out for a deliberately public endpoint. Above it: `#[use_guards(...)]` on the host struct and per operation, plus a per-operation posture `#[authorize(Action, Entity)]` / `#[public]` — **mandatory: no posture ⇒ compile error**, as on GraphQL | `[<Feature>Module, AppAuthzMcpModule]` |

### Why GraphQL uses a marker but WS binds real guards

HTTP guards run on `&mut Request` before the handler — they *are* the
auth chain.

**GraphQL** runs authn/ability **in-band** per operation, then seeds
`Ability` into per-operation context and forwards the principal with
`forward_principal!(Claims)`. That forward declares no owner and needs none:
it copies `Claims` off the request only when the in-band chain attached them,
so omitting `AppAuthzGraphqlModule` means the chain never runs and there is
nothing to copy. The marker this used to require was an empty struct in every
consumer, and silent when forgotten — see the *Hard "no" list*'s module-gating
entry.

**WS** instead reuses the connection **upgrade** (an HTTP `GET`), so the
gateway binds the real HTTP guards on its struct; they run once at
upgrade and are access-graph-validated the same way — omit
`AppAuthzWsModule` ⇒ those guards are unreachable ⇒ boot fails. Because the
upgrade's task-locals have unwound by the time a message handler runs,
`WsDataContext` re-seeds executor + ability around each message;
per-message `Guard`s (bound beside a `#[subscribe_message]`, reusing
`Guard::check_ws_message`) add event-level checks when needed. There is
**no** `WsAuthnGuard`/`MessageGuard` marker type — WS reuses the HTTP
`Guard` trait directly, and **no `WsAbilityBridge`** either: a gateway is
`EdgePosture::Guarded`, so there is no in-band chain to re-run. Hence the one
asymmetry worth stating: on GraphQL and MCP the operation *guard* installs the
ambient ability, on WS the *data context* does. `nest_rs_authz::ws::authorize`
reads it the same way regardless, which is what lets one `#[authorize]` mean one
thing on all four transports.

**Above that, WS declares the same posture the other two do.** `#[messages]`
emits, per message: the gate (`nest_rs_authz::ws::authorize`) before the payload
is deserialized, then the argument pipes, then the reply mask
(`nest_rs_authz::ws::masked_reply_for`). Mandatory, for the same fail-secure
reason it is on a `#[query]`.

**But WS masks like HTTP, not like MCP** — and that is a decision, not an
oversight. GraphQL and MCP mask *and reconstruct* the operation's return type, so
a stripped required key fails the operation (a non-nullable field; rmcp's
`structuredContent`). A WS envelope carries whatever JSON the value serialized to
and promises no schema, so the masked key is simply **absent from the frame**,
exactly as `RouteResponseShaper` omits it from a body. Hence no `DeserializeOwned`
on the reply type, and a field-restricted caller reads a smaller object rather
than an error. What stays fail-closed is what should: no ambient ability, and a
body that cannot be reconciled with the entity at all.

One WS-only compile rule remains, and its reason is *not* the fail-closed one: a
masked message returns a **literal** `Result<T, E>`, because the reply-shape
decision is syntactic — a `Result` behind an alias reads as an ordinary value and
would be masked as the `Result`, producing a success-shaped frame carrying
`{"Ok": …}`.

**MCP mirrors GraphQL, one seam for one seam.** Both are
`EdgePosture::Exempt`, so both gate in-band through their own operation
guard: same authn→authz ordering (`nest_rs_authz::run_ability_chain` — one
function, two error mappings), same global-pool fallback when no bridge is
registered (`FallbackMcpGuard` / `FallbackOperationGuard`), and the *guard's*
`around` installs the ambient ability on both. Two deliberate differences:
`/graphql` carries the `Public` marker so a pooled `AppAuthnGuard` admits an
anonymous operation through to the resolver gates, and `/mcp` does not — an
unauthenticated tool call is refused; and MCP's *no-pool* tail is deny-all
rather than pass-through, so the fallback can only ever widen what
`use_guards_global` opted into.

**Above the bridge, MCP declares the same layers GraphQL does.** `#[tools]` on
the host's impl emits, in order: the guard chain (`run_layered_mcp_chain`), the class gate
(`nest_rs_authz::mcp::authorize`), the argument pipes, and the response mask
(`nest_rs_authz::mcp::masked_value_for`). Posture is mandatory per operation for
the same fail-secure reason it is on a `#[query]`. Two deliberate MCP-only
differences, both forced by the protocol: masking has no selection set to excuse
a stripped required field, so it fails **closed** (`unmasked` is the opt-out for
a deliberate projection); and `bind = Service` does not carry over — an operation
takes one `Parameters<T>` struct, not the named id argument the binding reads.

**The MCP bridge gates every capability, not `tools/call`.** rmcp 3.x replaced
its single dispatch method with a `ServerHandler` trait, so `PropagatingHandler`
delegates the whole surface and applies guard + ability + transaction per
operation. `prompts/get`, `resources/read`, `completion/complete` and `tasks/*`
are therefore scoped exactly like a tool call; the two documented exceptions are
notifications and the long-lived `subscriptions/listen`.
`crates/nest-rs-mcp/tests/integration/propagate.rs` is what keeps a future
rmcp method from silently escaping the chain — extend it when rmcp grows one.

## Bound mutations (GraphQL)

A bound mutation receives its subject as an `Authorized<A, E>` parameter,
but **the posture stays explicit**: write `#[authorize(Action, Entity)]`
and load the subject in the body with `bind_required::<A, Service>(ctx,
&id)`, or use the `#[authorize(A, bind = Service)]` form
(container-resolved service) which binds the `Authorized<A, E>` subject
for you.

A bound subject's `Authorized<A, E>` proof is **action-typed**: a `Read`
proof cannot be passed where an `Update` proof is required — a compile
error, not a runtime surprise.

## Public handlers

Omit `#[use_guards(...)]` for that transport and lose the transitive
`Authz<Transport>Module` import — **the app must list it explicitly** if
other handlers need it.
