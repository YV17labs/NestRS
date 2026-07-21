# 1.0 Punchlist — execution plan

Work order for the pre-1.0 hardening pass. Written for agent sessions that
execute tasks **without re-deriving the analysis**: every task below was
verified against the code (file:line evidence), decisions that were open have
been made, and the compass for any remaining judgment call is stated. Follow
the plan; do not reopen the decisions.

**Delete this file in the final task, before tagging 1.0** (Phase G).

---

## Ground rules (read before any task)

1. Read `CLAUDE.md` first. Its hard-"no" list, naming table, test-layout norm
   and observability rules apply to every task here. When this punchlist and
   `CLAUDE.md` seem to conflict, stop and ask the owner.
2. **Compass for judgment calls**: the best solo-developer framework —
   runtime performance, developer elegance (less ceremony, more transparency),
   and normed cleanliness, in that spirit. Churn is acceptable; a public
   API that is wrong at 1.0 is not.
3. **Definition of done** applies per task, per workspace touched
   (see "Verification gates" at the bottom). Show evidence — paste the
   command tails, never assert success.
4. The devcontainer's e2e infra (Postgres, Redis, S3) is always reachable.
   Never skip e2e claiming it is not; a connection failure is a regression
   to report.
5. **Progress rule**: two consecutive attempts with no measurable progress
   toward a task's verification → stop, report the blocker, move on.
6. Commits: conventional style matching `git log` (`feat(framework):`,
   `fix(demo):`, `docs:`, …), one commit per task or coherent task group.
   No tool attribution or generated-by trailers.
7. Do not touch CI/CD. Out of scope for this entire punchlist.

### Execution order and parallel sessions

Phases are ordered by dependency. If running parallel sessions, use this
split — it avoids file conflicts:

| Session | Phases | Constraint |
|---|---|---|
| 1 | **A** then **B** | A before B (same macro crates). A1 also edits `demo/Cargo.toml` pins — land A1 before session 3 starts, or rebase. |
| 2 | **F** | Independent (docs, CLI, metadata). Can start immediately. |
| 3 | **C** then **D** then **E** | Demo workspace only. Serial within the session. |
| — | **G** | Last, single session, after everything merged. |

---

## Phase A — Semver freeze (framework public surface)

Everything `pub` at 1.0 is a promise. Each task here is cheap now and
breaking after release. **This phase blocks the tag.**

### A1. Re-export `sea_orm`; fix demo dependency skew — BLOCKER

**Why.** The root `Cargo.toml` "Pinned-major policy" declares sea-orm part of
the public API contract (exact-pinned `=2.0`), and `nest-rs-http` re-exports
`poem` (`crates/nest-rs-http/src/lib.rs:44`), `nest-rs-graphql` re-exports
`async_graphql` — but no crate re-exports `sea_orm`. Every consumer must
carry its own dep and mirror an exact pin by hand. The failure mode is
already live in this repo: `demo/Cargo.toml:123` still pins
`sea-orm = "2.0.0-rc.43"` against the framework's `=2.0`.

**Do.**
- Add `pub use sea_orm;` at the root of `crates/nest-rs-seaorm/src/lib.rs`,
  with a short doc line mirroring how `nest-rs-http` documents its `poem`
  re-export (lockstep rationale).
- Fix `demo/Cargo.toml`: `sea-orm` → the same exact pin as the framework
  (`=2.0...` — copy the framework's spec verbatim).
- Same file: `async-graphql = "7"` (around line 104) → the framework's exact
  pin (`=7.2.1`). Same lockstep hazard, milder.
- Migrate demo imports opportunistically only if trivial; the re-export is
  the deliverable, not a demo-wide import rewrite.

**Verify.** Framework gates + `demo/` gates (both workspaces touched).
`grep -rn "rc.43" demo/` returns nothing.

### A2. Re-export `rmcp` in `nest-rs-mcp`

**Why.** MCP is the only transport forcing a direct third-party dep on apps.
`crates/nest-rs-mcp/src/lib.rs:70-79` re-exports selected rmcp items, but
rmcp's macros expand to `rmcp::` paths, so an app hosting a tool must add
`rmcp = "2.2"` itself — `demo/crates/features/Cargo.toml:27-30` documents
exactly this workaround. Queue/WS keep apps free of `serde_json`/`tracing`
via hidden re-exports; MCP should match. This is the "framework carries the
rest" thesis applied to its own dependency surface.

**Do.**
- Add `pub use rmcp;` to `crates/nest-rs-mcp/src/lib.rs` (keep the existing
  curated item re-exports — they are the documented API; the wholesale
  re-export is the macro-expansion escape hatch).
- In `demo/crates/features/`, drop the direct `rmcp` dep if the expansion can
  resolve through `nest_rs_mcp::rmcp` (check how the rmcp macros qualify
  paths; if they hard-code `::rmcp`, the direct dep must stay — in that case
  keep it and update its comment to state the lockstep contract plainly, and
  note the finding in the A2 commit message).

**Verify.** Framework + demo gates; the assistant app's MCP e2e
(`demo/apps/assistant/tests/e2e/`) must pass.

### A3. Error-naming pass — decided, minimal churn

**Decisions already made — do not re-litigate:**
- `nest_rs_redis::ConnectionError` → **rename to `RedisError`**
  (`crates/nest-rs-redis/src/error.rs:16`, re-export at `lib.rs:49`). Generic
  infra-error names collide in app imports; the house pattern is
  concern-prefixed (`ConfigError`, `StorageError`, `QueueError`).
- `nest_rs_seaorm::ServiceError` — **keep the name.** It is a
  developer-vocabulary type written in every app service signature
  (`Result<T, ServiceError>`); it is role-named like `Service` itself, and a
  concern-prefixed rename (`SeaOrmError`) would hurt exactly the ergonomics
  the framework sells. Add one sentence to the `error.rs` module doc stating
  the naming decision so it never gets re-flagged.
- Bare `Result` aliases at crate roots (`crates/nest-rs-config/src/lib.rs:21`,
  `crates/nest-rs-storage/src/lib.rs:54`) — **keep.** `io::Result` /
  `fmt::Result` is the blessed std idiom; Rust-first wins over the style nit.
  No action.

**Do.** The `RedisError` rename (definition, re-export, all uses in
framework and demo), plus the one-line `ServiceError` doc note.

**Verify.** Framework + demo gates. `grep -rn "ConnectionError" crates/ demo/
--include=*.rs` returns nothing.

### A4. `#[doc(hidden)]` pass — stop internals from becoming promises

**Why.** Macro-support re-exports and cross-crate wiring seams are `pub` by
necessity, but without `#[doc(hidden)]` they render as documented API and
freeze at 1.0. `nest-rs-queue` already does this correctly
(`crates/nest-rs-queue/src/lib.rs:46-53`) — replicate that treatment.

**Do.**
- `crates/nest-rs-ws/src/lib.rs:95-96` — `pub use serde_json;` /
  `pub use tracing;` → add `#[doc(hidden)]` (copy the queue crate's comment
  style).
- `crates/nest-rs-graphql/src/lib.rs` — `pub use inventory;` → same.
- `crates/nest-rs-http/src/lib.rs:31-52` — the wiring seams (`SchemaFn`,
  `schema_of`, `HttpEndpointWrap`, `endpoint_wrap_priority`, `MaskProbe`,
  `MaskProbedEndpoint`, `mask_probed`, `shaped`, `SelfMountGuardWrap`):
  either `#[doc(hidden)]` each, or group them under a `#[doc(hidden)] pub mod
  seam;` — pick whichever keeps sibling-crate imports stable with less churn.
- `crates/nest-rs-core/src/lib.rs` — same treatment for `compose_chain`,
  `dedup_bucket`, `ResolvedLayer`. **Exception:** if A5 makes `ResolvedLayer`
  part of the deliberate public layer vocabulary, leave it visible — decide
  inside A5, not here.
- `crates/nest-rs-core/src/lib.rs` — `__module_registered` is already
  `#[doc(hidden)]` at definition; move its re-export out of the curated list
  visually (own line, comment "macro plumbing").
- `crates/nest-rs-codegen/src/lib.rs:25` — `injected_keys_expr` has zero
  external callers (verified): make it `pub(crate)` (or delete the export).
  The rest of codegen's single-consumer exports stay — the crate's charter
  (third-party decorator authors) covers them.

**Verify.** Framework gates. Then `cargo doc -p nest-rs-http -p nest-rs-ws
-p nest-rs-graphql -p nest-rs-core --no-deps` and confirm the hidden items no
longer appear in the rendered index.

### A5. Unify the five Layer-System `*Spec` families — the one real refactor

**Why.** The `{ type_id, name, resolve }` struct + constructor-fn +
newtype-vec trio is copy-pasted five times: `GuardSpec`/`guard()` and
`PipeSpec`/`pipe()` (`crates/nest-rs-guards/src/registry.rs:15-65`),
`FilterSpec` (`crates/nest-rs-filters/src/registry.rs:13-47`),
`InterceptorSpec` (`crates/nest-rs-interceptors/src/registry.rs:13-47`),
`ExceptionFilterSpec` (`crates/nest-rs-exception-filters/src/registry.rs:17-57`).
Doc phrasing has already diverged between copies. The generic shape exists
twice already: `ScopedLayerSpec<L: ?Sized>`
(`crates/nest-rs-guards/src/dispatch/scoped_spec.rs:22`) and
`ResolvedLayer<L: ?Sized>` (`crates/nest-rs-core/src/layer_chain.rs:33`).
These five structs are field-public API — after 1.0 they are frozen forever.

Additionally, the fail-secure boot check ("global X not resolvable from the
container ⇒ fail boot naming it") is copy-pasted five times:
`crates/nest-rs-guards/src/builder.rs:104-119` and ~175,
`crates/nest-rs-filters/src/builder.rs:51-70`,
`crates/nest-rs-interceptors/src/builder.rs:58-77`,
`crates/nest-rs-exception-filters/src/builder.rs:34-53`. Five copies of a
security net means a future hardening lands in one family and silently not
the others.

**Do.**
1. In `nest-rs-core`, beside `ResolvedLayer` in `layer_chain.rs` (core is the
   documented "single dedup logic for all five families"), add a generic
   `LayerSpec<L: ?Sized>` carrying exactly the shared shape, plus the generic
   constructor fn and the newtype-vec if it can be made generic cleanly.
2. In each of the five crates, replace the local struct with a type alias
   preserving every existing public name and constructor signature:
   `pub type FilterSpec = LayerSpec<dyn Filter>;` etc. **Zero call-site
   churn outside the five registry files is the acceptance bar** — macro
   crates and app code must compile unchanged.
3. Extract the boot check into one shared helper (e.g.
   `check_specs_resolvable(specs, kind) -> HttpBootCheck` or equivalent) —
   it falls out nearly free once the generic spec exists. Keep the per-family
   error message content (kind name, consequence phrasing) identical to
   today's strings; this is a dedup, not a reword.
4. Reconcile the doc-comment drift: one canonical doc on the generic, aliases
   get one-liners.

**Do NOT.** Do not change dispatch semantics, ordering (band numbers), or
the deny-all/fail-secure behavior. Do not merge `ScopedLayerSpec` into this
unless it is a pure win with zero behavior risk — it has a different
`resolve` shape; out of scope if in doubt.

**Verify.** Framework gates including the seaorm/storage e2e suite
(`cargo nextest run --workspace -E 'binary(e2e)'`), plus full demo gates —
the demo is the best integration test of the layer system. Also run
`demo` e2e (`cd demo && nestrs run test e2e`).

### A6. Kill the one hot-path panic: PK-less entities in seaorm

**Why.** `crates/nest-rs-seaorm/src/page.rs:129` and
`crates/nest-rs-seaorm/src/service.rs:441` both
`.expect("an entity has at least one primary-key column")` on the query and
mutation hot paths. SeaORM permits PK-less entities (views, raw tables); a
user modeling one gets a mid-request panic instead of an error, in the layer
whose written contract is "never panic, return `DbErr`". Contrast:
`crates/nest-rs-resource-macros/src/relations.rs:57-63` correctly makes the
same condition a compile error on its own path.

**Do.** Prefer failing at the earliest boundary available:
- If the call sites can know the entity type at macro/boot time, emit a
  compile or boot error (mirroring `relations.rs`) — best.
- Otherwise, replace the `expect` with a typed error: `ServiceError::Internal`
  (or a dedicated variant) carrying a message naming the entity, logged at
  `error` with structured fields. Never a panic, never a silent default.
- Add a unit test exercising a PK-less mock entity if SeaORM's test surface
  allows constructing one cheaply; if it does not, state that in the commit
  message rather than forcing an artificial test.

**Verify.** Framework gates + seaorm e2e suite.

### A7. WS `Scoped` parity — decided: implement, with a defined fallback

**Why.** `nest_rs_http::Scoped` (`crates/nest-rs-http/src/lib.rs:41`),
`nest_rs_graphql::Scoped`, and `nest_rs_mcp::Scoped` all exist;
`crates/nest-rs-ws/src/` has no `scope` module, so
`#[injectable(scope = request)]` providers are unreachable from WS message
handlers — an arbitrary asymmetry in the four-transport story.

**Decision.** Parity wins (compass: transparency and elegance). Implement
`Scoped` for WS with **per-message** scope: each inbound message dispatch
opens a `RequestScope` (core's existing type), the same way guards already
run per message. Model the implementation on `nest-rs-mcp`'s `scope.rs`
(the most recent transport to add it), not HTTP's.

**Fallback (use only if the investigation shows it).** If WS message dispatch
has no seam where a per-message scope can be installed without restructuring
the pipeline (i.e. this stops being a small task), do NOT force it: instead
document the asymmetry as a deliberate 1.x item in the `nest-rs-ws` crate
doc (`//!` header — connection ≠ request, scoped providers arrive in 1.x)
and add it to `ROADMAP.md` under the appropriate section. Either outcome
closes the task; silent asymmetry is the only failure.

**Verify.** If implemented: framework gates + a WS e2e in the demo's live app
exercising a request-scoped provider from a message handler
(`demo/apps/live/`), + demo gates. If fallback: framework doc build clean;
ROADMAP updated.

---

## Phase B — Codegen dedup (framework internal)

All `pub(crate)`-level; fixable post-1.0, but the copies have **already
diverged** and they gate security-relevant surface. Run after Phase A
(same crates). The repo's own rule (`.claude/rules/framework.md`: "Shared
token helpers in `nest-rs-codegen`") is the mandate.

### B1. Move the quadruplicated attribute helpers into `nest-rs-codegen`

Four helpers, three copies each, already drifting:
- `take_use_attr` — `crates/nest-rs-http-macros/src/attr.rs:38`,
  `crates/nest-rs-ws-macros/src/attr.rs:40` (byte-identical), and
  `crates/nest-rs-graphql-macros/src/resolver.rs:156` as `take_path_list`
  (same body; error string diverged: "list every entry in it" vs "list every
  guard in it").
- `take_flag_attr` — `nest-rs-http-macros/src/attr.rs:26` and
  `nest-rs-graphql-macros/src/resolver.rs:175` (identical).
- `expr_str` — `nest-rs-http-macros/src/attr.rs:8` and
  `nest-rs-ws-macros/src/attr.rs:7` (identical).
- `reject_http_only_layers` — `nest-rs-ws-macros/src/attr.rs:21` and
  `nest-rs-graphql-macros/src/resolver.rs:131` (same logic; only the
  transport noun differs — parameterize it).

**Do.** New module in `nest-rs-codegen` (e.g. `src/args.rs`), move the four
helpers there (all three macro crates already depend on codegen — verified),
delete the local copies including the ws copy's "local copy so this crate
stays free of a dep on nest-rs-http-macros" comment (the rationale was
wrong: the sanctioned home was always codegen). Where copies diverged, keep
the **more specific** error message per call site by passing the noun as a
parameter ("guard"/"entry"). These helpers gate `use_guards` / `force_guards`
/ `public` — behavior must be identical before/after; if any copy's behavior
genuinely differs beyond the message string, stop and report instead of
picking silently.

**Verify.** Framework gates + demo gates (the demo compiles every macro).

### B2. Collapse the 12 `ScopedLayerSpec` emission functions

Twelve near-identical token-emission functions, parameterized only by the
erased dyn-trait path: `crates/nest-rs-http-macros/src/routes.rs:615, 633,
653, 683, 702`, `crates/nest-rs-http-macros/src/controller.rs:203, 222, 242,
260, 280`, `crates/nest-rs-graphql-macros/src/resolver.rs:507`. Divergence
already present: empty case is `::std::vec![]` in `routes.rs` but
`::std::vec::Vec::new()` in `controller.rs`.

**Do.** One helper in `nest-rs-codegen`, e.g.
`scoped_specs(paths: &[syn::Path], erased: TokenStream) -> TokenStream`;
replace all twelve. Pick ONE empty-case form. Also dedupe
`force_guard_typeids` (identical at `routes.rs:670` and `resolver.rs:524`).

**Verify.** Framework + demo gates. The emitted tokens must be semantically
identical — if any of the twelve turns out to differ beyond the erased path
and the empty-case form, stop and report.

### B3. Delete `to_snake`

`crates/nest-rs-events-macros/src/listeners.rs:175` is character-identical
to `nest_rs_codegen::snake_case` (`crates/nest-rs-codegen/src/casing.rs:8`),
and `listeners.rs:21` already imports from codegen. Delete the local fn,
import `snake_case`. Trivial.

**Verify.** Framework gates.

---

## Phase C — Demo correctness and security — BLOCKERS

The demo is the showcase; these two are real defects in it.

### C1. Move the "already published" invariant into `PostsService::publish`

**Why.** The invariant is enforced per-transport instead of in the service:
HTTP calls `svc.ensure_unpublished(&model)` then `svc.publish(model)`
(`demo/crates/features/src/posts/http/controller.rs:73-76`, 409 tested at
`demo/crates/features/tests/e2e/posts/http.rs:239-251`), but GraphQL calls
`svc.publish(model)` directly
(`demo/crates/features/src/posts/graphql/resolver.rs:31-33`) — so a
published post can be re-published over GraphQL, re-emitting
`PostPublishedEvent` each time (duplicate worker notifications). Worse, the
e2e at `demo/apps/api/tests/e2e/posts.rs:217-224` republishes up to 5 times
over GraphQL and asserts OK — the suite depends on the missing check. This
is the tutorial feature teaching a transport-dependent business rule.

**Do.**
- Fold the check into `PostsService::publish` itself (publish on an already
  published post → the service's conflict error, e.g.
  `ServiceError::conflict(...)`, consistent with what
  `ensure_unpublished` produces today).
- Remove `ensure_unpublished` as a separate public step **if** nothing else
  needs it (one way to do a thing); the HTTP controller then just calls
  `publish` and maps the error as it already does for other conflicts.
- Fix the GraphQL e2e at `apps/api/tests/e2e/posts.rs:217-224`: it must now
  assert the **error** on republish (and keep one success on first publish).
  Keep the HTTP 409 e2e green.
- Check the MCP posts tool and any other publish path for the same
  by-pass; all transports must go through the one service method.

**Verify.** Demo gates including e2e (`nestrs run test e2e`). Evidence: the
republish test now asserts a conflict on both transports.

### C2. Stop the MCP audio tool leaking storage error detail

**Why.** `demo/crates/features/src/audio/error.rs:1-8` documents that
storage/queue `Display` output contains endpoint hostnames and connection
detail, so wire bodies must stay opaque — the HTTP controller honors this.
But `demo/crates/features/src/audio/mcp/tool.rs:39` does
`.map_err(|e| McpError::internal_error(e.to_string(), None))`, putting that
same `Display` on the MCP wire. (`posts/mcp/tool.rs:29` is fine —
`ServiceError`'s Display is the constant `"database error"`.) "Security is
primordial."

**Do.** Mirror the HTTP controller's opaque mapping: constant client-facing
message on the wire, full error to `tracing` at `error` (or `warn` if it is
a denial) with structured fields (`error = %e`), target per the demo's app
conventions. If Phase C3 lands first, the typed error's opaque `Display`
does this naturally — coordinate.

**Verify.** Demo gates; the assistant/MCP e2e still passes; add or extend a
test asserting the wire message is the constant string when storage errors
(if a storage failure can be provoked in e2e cheaply — e.g. unknown key;
otherwise a unit test on the mapping).

### C3. `AudioService`: `anyhow` → typed error

**Why.** `demo/crates/features/src/audio/service.rs:36,51,60,76,87,102,107`
returns `anyhow::Result` from a lib crate — against the posture (thiserror
in libs, anyhow at app entry) and every other feature's practice. It also
forces `map_err(|e| storage_error(...))` ceremony in all four HTTP handlers
(`demo/crates/features/src/audio/http/controller.rs:50,70,93,115`).

**Do.** Define `AudioError` in the existing
`demo/crates/features/src/audio/error.rs` (thiserror; variants for
storage/queue/db as the flows need; opaque `Display` per the module's own
doc — the detail goes to tracing, not the wire). Convert the service's
signatures, delete the handler-side `map_err` ceremony by giving
`AudioError` the same response-mapping treatment other feature errors get
(look at how `users`/`posts` map errors — copy the exemplar, don't invent).
This directly enables C2's clean fix.

**Verify.** Demo gates + audio e2e.

---

## Phase D — Demo conformity (pattern drift vs the `users/` exemplar)

The demo's story is "copy `users/` and go" — drift between features costs
credibility. Run after Phase C (same files in `audio/`).

### D1. Two fat suite `main.rs` files — locked-norm violation

The test-layout norm (locked): suite `main.rs` stays thin (`//!` + `mod`).
- `demo/apps/worker/tests/e2e/main.rs` — the whole suite (queue, processor,
  module wiring, test) lives inline.
- `demo/crates/seed/tests/e2e/main.rs` — the test is inline.

**Do.** Split each into modules mirroring `src/` (norm rule 3); `main.rs`
keeps only `//!` and `mod` lines. Pure moves — no test-logic changes. Use
`demo/crates/features/tests/e2e/` as the shape reference.

**Verify.** `cd demo && nestrs run test e2e` — same tests, same results.

### D2. `audio/dto.rs` → `audio/dtos/`

Five transfer types in one bare file
(`demo/crates/features/src/audio/dto.rs`: `TranscodeDto`,
`UploadRequestDto`, `PresignedUrlDto`, `TranscodeState`,
`TranscodeEventDto`) violate the same-role-plural rule. `oauth/dtos/` and
`demo/apps/live/src/chat/dtos/` show the target shape: pluralized folder,
one `<snake>_dto.rs` per type, `mod.rs` as pure index.

**Do.** Mechanical split + re-export so call sites keep their import paths
via the folder's `mod.rs`. `TranscodeState` is a state enum, not a DTO — if
it is used beyond the DTOs, consider whether it belongs in the feature root
instead; keep the move minimal either way.

**Verify.** Demo gates.

### D3. App-composition drift across the five apps

The five apps are the canonical composition showcase and must be uniform:
- `demo/apps/assistant/src/main.rs` — only app missing `Environment::init()`
  (no `.env` cascade); `assistant/src/module.rs` — only HTTP app missing
  `ConfigModule::for_root()`. Add both, matching `apps/api` line-for-line in
  style.
- `demo/apps/worker/src/module.rs` — `main.rs` calls
  `OpenTelemetry::init("worker")` but the module never imports
  `OpenTelemetryModule`; and worker is the only app with no `HealthModule` —
  the one deployable most likely to run under an orchestrator, with no
  liveness probe. Wire both, following `apps/api`'s module list.

**Do NOT** invent new composition patterns; copy `apps/api/src/module.rs`
(the documented canonical composition) for each missing piece.

**Verify.** Demo gates + e2e. For the worker health endpoint: run the worker
(`cd demo && nestrs run` or the worker's dev recipe), curl the health route,
paste the response, kill the process before returning control.

### D4. Posts GraphQL resolver: use the exemplar's binding seam

`demo/crates/features/src/posts/graphql/resolver.rs:30-35` hand-parses the
Uuid, calls `CrudService::access`, and maps `Denied ⇒
Error::new("forbidden")`, while the reference
`demo/crates/features/src/users/graphql/resolver.rs:39` uses
`bind::<Service, A>` (and `.claude/rules/authn-authz.md` names
`bind_required` / `#[authorize(A, bind = Service)]` for bound mutations).
Two ways to do the same thing across the two exemplar features.

**Do.** Rewrite the posts resolver's publish mutation on the `bind` seam,
exactly as `users` does. Behavior must not change (same authz result, same
error mapping) — after C1, the republish-conflict comes from the service.

**Verify.** Demo gates + the posts GraphQL e2e (including the org-B
row-level filtering assertions, which must stay green).

### D5. Move `relational_authz.rs` into the framework's own suite

`demo/crates/features/tests/e2e/relational_authz.rs` tests framework
behavior with synthetic `container`/`item` entities inside the product
suite — it breaks "module tree mirrors `src/`" and is a framework e2e in
the wrong repo layer. **Decision: move it** to `nest-rs-seaorm`'s e2e suite
(`crates/nest-rs-seaorm/tests/e2e/`, respecting that suite's existing module
layout). Port the synthetic entities with it; strip any demo-feature imports
(the framework never names concrete product types — generic or synthetic
fixtures only). If the test turns out to depend on demo-only wiring that
cannot be reproduced framework-side without significant new fixture code,
stop and report what it needs instead of forcing it.

**Verify.** Framework e2e suite green (`cargo nextest run --workspace -E
'binary(e2e)'`), demo e2e still green after the removal.

### D6. Small conformity nits (one commit)

- `demo/crates/seed/src/bin/seed.rs:7` uses `println!`; the sibling
  `migrate` binary uses tracing. Align seed on tracing (structured fields,
  constant messages).
- `demo/crates/features/src/oauth/scope.rs:3-8`: `user.role` is a raw
  `String` column with a silent unknown→`User` fallback in `role_from_db`.
  Strict-typing rule prefers the enum-column pattern `PostStatus` already
  demonstrates. **Decision: convert to a DB-backed enum** like `PostStatus`
  (fallback direction was at least least-privilege, but silent coercion of
  an unknown role value is exactly the kind of dust this pass removes; an
  unknown value should be a load error, not a quiet demotion). This needs a
  migration — additive conversion only; if the conversion cannot be done
  without rewriting existing rows' semantics, stop and ask (migrations that
  rewrite data are owner territory).
- `demo/crates/features/src/audio/http/extract.rs` — `extract.rs` is not in
  the naming table. **Decision: keep the file, name it as the concern**
  (custom extractor); no rule change needed — but confirm the file contains
  only the extractor. If it accumulates other roles, split per the table.

**Verify.** Demo gates; e2e if the role-enum migration lands.

---

## Phase E — Demo showcase gap: transactions

**Why.** The framework's headline transparent concern — per-request lazy
transactions, committed on 2xx, rolled back otherwise
(`crates/nest-rs-seaorm/src/http/interceptor.rs`) — is demonstrated nowhere:
no demo test asserts rollback semantics (grep for rollback/transaction in
demo tests: zero hits), and no feature performs a multi-write operation. The
one hard part the showcase dodges.

### E1. Make `publish` a two-write operation and assert rollback end-to-end

**Do.**
1. Add a second write to the publish flow — e.g. a `post_publications` audit
   row (post id, actor, published_at) inserted by `PostsService::publish` in
   the same ambient request transaction as the status UPDATE. New table via
   a new additive migration in `demo/crates/migrations` (follow the existing
   migration style). Keep it small: this exists to showcase atomicity, not
   to grow the product.
2. e2e (in the features or api suite, wherever the publish e2e lives):
   - Happy path: publish → both rows present.
   - Rollback path: force the **second** write to fail with a real DB error
     — e.g. a unique constraint on `post_publications.post_id` plus a
     pre-inserted conflicting row via the test's seed/repo setup — then
     assert the response is an error AND the post's status is still
     unpublished (first write rolled back). **No DB mocking** (hard rule);
     the failure must come from the real Postgres constraint.
3. If C1 landed, ensure the conflict-check and the audit-row constraint
   don't shadow each other in the test (the rollback test must reach the
   second write: publish a *fresh* post whose audit insert conflicts).

**Verify.** Demo gates + full e2e; paste the rollback test's output as
evidence. This is the flagship proof — it deserves a doc sentence in the
posts feature's `//!` or the demo README if one naturally fits (one
sentence, not a chapter).

---

## Phase F — Text and credibility sweep

Independent of all other phases; safe to run in parallel. The audience is a
reader deciding whether to trust 1.0.

### F1. Rewrite the guards crate header — RELEASE BLOCKER

`crates/nest-rs-guards/src/lib.rs:95-100` says "HTTP-coupled for 0.x,
deliberately … the transport-neutral core trait is scheduled before 1.0
(see ROADMAP)". `ROADMAP.md:141-147` has since re-decided this as a
deliberate **1.x** design ("lands in a major"). On release day, page one of
the guards docs would announce a broken pre-1.0 promise. Rewrite the
paragraph to the ROADMAP's confident framing: HTTP-coupled is the deliberate
1.x design, the cost is binary size only, no security effect, transport-
neutral core trait is a planned major-version evolution. Keep it to a few
sentences; no apology.

### F2. Scrub the authn crate header

`crates/nest-rs-authn/src/lib.rs:3-4`:
- Line 3 cites `tests/authn.rs` — a path that doesn't exist and a flat
  layout the locked norm forbids; it also references CLAUDE.md (internal
  doc) from published rustdoc. Fix the path (`tests/integration/main.rs`)
  and drop the CLAUDE.md pointer.
- Line 4's "Gaps: …" untested-areas list does not belong on docs.rs page one
  of an **auth** crate. Delete the line; if the owner wants the list kept,
  it moves to an issue tracker, not shipped rustdoc.

### F3–F5. Metadata fixes (one commit)

- `crates/nest-rs-openapi/Cargo.toml` description says "OpenAPI 3.0"; the
  code emits 3.1 (`crates/nest-rs-openapi/src/document.rs:68`, lib.rs header
  says 3.1). Fix the description.
- `README.md:13-14` — duplicated identical MIT badge. Remove one.
- Add `keywords` and `categories` to every `nest-rs-*` `Cargo.toml`
  (workspace-inherit where possible). Choose accurate crates.io categories
  (e.g. `web-programming`, `asynchronous`, `database`) and ≤5 keywords per
  crate (`framework`, `nestjs`, plus per-crate concern words). Do not
  keyword-stuff; accuracy over reach.

### F6. Neutralize the private fixture string

`crates/nest-rs-seaorm/src/slug.rs:131` — the slugify test fixture contains
the client gallery's real name (a private reference in a public repo).
Replace with a neutral accented fixture that preserves the accent-folding
coverage (e.g. `"Galerie Métropole"` → `"galerie-metropole"`). Check the
whole file for further occurrences.

### F7–F9. Stale text (one commit)

- `crates/nest-rs-schedule/src/lib.rs:12` cites `tests/end_to_end.rs` — the
  real layout is `tests/integration/`; "end_to_end" is also a forbidden
  suite name. Fix the path in the comment.
- `crates/nest-rs-interceptors/src/lib.rs:39` — the crate-level doc example
  teaches `.parse().unwrap()` inside an interceptor, the exact hot-path
  pattern the rules forbid, shipped as the copy-paste exemplar. Rewrite the
  example to propagate or map the error idiomatically.
- Brand casing in Cargo descriptions: most say "for nestrs";
  `nest-rs-database`, `nest-rs-pipes`, `nest-rs-queue`, `nest-rs-storage`,
  `nest-rs-throttler` say "for NestRS". **Decision: uniform "for NestRS"**
  (the display brand, 326 uses across md/mdx) in all descriptions.

### F10–F11. Doc↔code drift — fix the text side (code is the source of truth)

- Root `Cargo.toml`: the croner dependency comment references
  `#[cron_job(cron = "...")]`; the shipped surface is `#[scheduled]` +
  `#[cron("...")]` (`crates/nest-rs-schedule-macros/src/lib.rs`). Fix the
  comment. Also check `ROADMAP.md:148` ("Per-job transactions — a
  `#[cron_job]`/`#[processor]` …") for the same stale decorator name.
- `.claude/rules/apps.md` cites `assistant/weather/` as the app-local
  feature example; no such folder exists. Point the example at a real
  app-local feature (`apps/live/src/chat/` or `apps/live/src/notify/`).

### F12. CLI cleanup (one commit)

- `crates/nest-rs-cli/src/templates/adapter.rs:93` — the WS gateway scaffold
  emits `let _ = client.broadcast(...)` into user code: the generator
  teaches the silent-failure anti-pattern the hard-no list forbids. Emit an
  idiomatic handling instead (propagate with `?` if the generated fn returns
  `Result`, else log at `warn` with structured fields and a comment saying
  why delivery failure is tolerable here). Regenerate/adjust any template
  snapshot tests.
- `crates/nest-rs-cli/src/commands/doctor.rs:37` — `if let Ok(Some(_))`
  swallows the discovery error, so a broken manifest reports
  `in_nestrs_workspace: false`. A diagnostic command must surface the
  diagnosis: distinguish "not a workspace" from "workspace detection failed:
  <err>".
- `crates/nest-rs-cli/src/naming.rs:242,265,278` — three
  `#[allow(dead_code)] // reserved:` functions (`dto_file`, `event_file`,
  `input_file`) for generators that do not exist. **Decision: delete them**
  (same spirit as the banned phantom feature flags; re-add with the
  generator when it ships). Remove their unit tests with them.
- `crates/nest-rs-cli/src/scaffold/transaction.rs:200` —
  `let _ = Command::new("rustfmt")`: print a one-line notice when rustfmt
  fails ("generated code left unformatted: <err>"). Best-effort stays
  best-effort, silently is the only wrong part.
- `crates/nest-rs-config/src/dotenv.rs:61-63` — a present-but-unreadable
  `.env` (permissions, invalid UTF-8) is treated as absent, and malformed
  lines are skipped silently. Emit `warn` with structured fields (path,
  error) for the unreadable-file case; `warn` once per file for skipped
  malformed lines. Config silently vanishing is a debugging tarpit.

### F13. Observability nits (one commit)

- `crates/nest-rs-opentelemetry/src/init.rs:166-173` — `Drop` runs
  `let _ = p.shutdown()` on tracer/meter/logger providers; a failed final
  flush silently loses telemetry. `Drop` can't return, but write the error
  to stderr (`eprintln!`) — tracing itself may be mid-teardown.
- `crates/nest-rs-opentelemetry/src/interceptor.rs:59` —
  `#[allow(unused_mut, unused_variables)]` blankets the whole `intercept`
  fn for a feature-combination artifact; scope it to the non-`otlp` build
  (`#[cfg_attr(not(feature = "otlp"), allow(...))]` or narrower) so the
  lints stay live for real regressions.

---

## Phase G — Final gates and release readiness

Single session, after all phases merged.

1. Full framework gates (all four commands, including both e2e filters).
2. Full demo gates (`nestrs run lint`, `test unit`, `test e2e`).
3. HTTP/GraphQL smoke: run the api app, curl one route per transport
   touched by this punchlist (posts publish over HTTP and GraphQL at
   minimum), confirm responses, kill the server.
4. `cargo doc --workspace --no-deps` clean; spot-check that A4's hidden
   items are gone and F1/F2's headers read correctly.
5. Re-grep the tripwires:
   `rg -n "rc\.43|ConnectionError|tests/authn\.rs|end_to_end\.rs|cron_job" --glob '!target' .`
   (expect: only intentional hits, e.g. ROADMAP historical notes — justify
   each remaining hit in the report).
6. Update `CHANGELOG.md` `[1.0.0]` with the user-visible changes from this
   pass (renames: `RedisError`; new re-exports: `sea_orm`, `rmcp`; WS
   `Scoped` if landed; demo publish semantics).
7. **Delete `PUNCHLIST-1.0.md`.** Report a summary of what shipped, what was
   skipped and why, and any stop-and-ask items that surfaced.

---

## Verification gates (copy-paste)

**Framework (root workspace):**

```sh
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
cargo nextest run --workspace -E 'not binary(e2e)' && cargo test --workspace --doc
cargo nextest run --workspace -E 'binary(e2e)'   # when seaorm/storage touched
```

**Product (`demo/`):**

```sh
nestrs run lint
nestrs run test unit
nestrs run test e2e      # when transports, DI wiring, or persistence touched
```

Baseline recorded 2026-07-21 before this punchlist: both workspaces pass
`clippy -D warnings` and `fmt --check` clean. Any new warning is yours.

## Stop-and-ask triggers (repeated from CLAUDE.md, verbatim intent)

Halt and surface the question rather than pick: anything on the hard-"no"
list; reopening a locked decision (test layout, workspace split, crate
naming); a **new third-party dependency**; a second way to do something a
decorator already does; a migration that drops or rewrites existing data; a
documented rule that has drifted from the code (report, don't edit either
side — except where a task above explicitly decides the direction).
