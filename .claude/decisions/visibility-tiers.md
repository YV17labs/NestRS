# Three visibility tiers: API, `__private`, `pub(crate)`

Through 7.0's foundation pass the extension surface was drawn by
`#[doc(hidden)]` alone: 154 hidden items at public paths, from a struct a
macro builds to a constant only a test reads, and nothing told a seam from
plumbing. A package author had to judge each item, and once 7.0 published,
sealing or promoting any of them would have been a break.

Now every crate an app links has three tiers. Everything public outside
`__private` is API. One inline `#[doc(hidden)] pub mod __private` per crate
holds what macro expansions and sibling crates call, says it is not API, and is
covered only by the lockstep `=` pins. The rest is `pub(crate)`.

- **The name is the ecosystem's** (`serde::__private`); a new word would
  have been invention where a convention exists.
- **Inline in `lib.rs`, re-exports only**, never a `__private.rs`: `lib.rs`
  holds `mod` and `pub use`, and that is all `__private` is.
- **An item in a public module sits in that file's `pub(crate) mod
  __private`.** Re-exporting it from the root alone would leave it reachable,
  and documented, at its module path (`nest_rs_core::trace_context::hex`).
  Making those modules private was refused: their paths are read
  (`operation_log::TARGET`, `layer_chain::GlobalSpecs`), and closing them is an
  API change of its own.
- **What Rust cannot place in a module keeps a `__` prefix and stays hidden** —
  a trait item, an exported macro — and a hidden inherent method becomes a free
  function in the type's own file, so private fields stay reachable without
  widening them.
- **A test-only export goes to `__private`**, never to a public path: an
  integration suite is a crate of its own and cannot see `pub(crate)`.
- **A hidden item that apps already use is API, not plumbing.** ws's `serde_json`
  and `tracing` were hidden, yet the demo and the WebSocket pages write
  `nest_rs::ws::serde_json::Value` and name `nest_rs::ws::tracing` as the
  gateway's logger; they were promoted rather than moved.
- An item a later change deletes (the HTTP endpoint wrap and its bands, the
  per-site guard chain cell) moves like any other, so no hidden item is left
  outside the tier while it waits. `KeyedDependency` is the one exception: the
  keyed-injection removal deletes it in place.

A per-item test of hiddenness was refused: no test reads source. Review and a
grep for a stray `#[doc(hidden)]` hold the tier; the hygiene build fails on an
emitted path that no longer resolves, and rustdoc's `-D warnings` on a doc link
that does. Review alone holds that a public doc names no `__private` item:
rustdoc resolves a link into a `#[doc(hidden)]` module without a warning, even
under `-D warnings`, and renders it as plain text or a dead path, so a public
doc says what such an item does in words.

**2026-10-19 — `SchemaFn` is API.** The route record became a read API whose
accessors hand the schema builders out, and nest-rs-openapi, its reader, named
their type through `__private`: a hidden alias in a public signature leaves
its reader nothing to look up. `nest_rs_http::SchemaFn` is tier 1;
`schema_of`, which only `#[routes]` emits, stays in `__private`.

**2026-10-27 — a public page's link into `__private` is a red check.**
"Review alone holds that a public doc names no `__private` item" is narrowed:
`just doc` now renders its public pass into an emptied doc folder, so no
private-pass page survives beside it, and fails on any rendered page whose
`href` names a `__private` path. rustdoc writes such a link as the bare path
(`href="crate::__private::endpoint"`), never as a page's URL, in its own crate
or across crates, so the check reads the name anywhere in the `href`. A link is
held by the build; a name written in a code span or in words, which rustdoc
renders as text, is still review's. The first run over the workspace found one
such link, `nest_rs_mcp::PropagatingHandler`'s builder, now said in words.

**2026-10-31 — the code is brought to the rule, and an item has one path.**
The identity audit found tier 1 holding what only expansions and sibling
crates call — the WebSocket edge's reply and error probes, which GraphQL and
MCP already kept here, the HTTP edge's path and version helpers, the global
pools' spec holders and `layer_chain::GlobalSpecs`, the trait they implement
(cited above as a read path; `layer_chain` stays public for `LayerSpec`),
MCP's `hosts_on` — and `async_trait` reachable at seven
paths, `#[input]` at five. With the owner's agreement to break the API before
7.0 publishes ("Tout faire", 2026-10-28), each moved to its crate's
`__private`; a whole-crate re-export a macro emits through
(`nest_rs_ws::nest_rs_http`, the scheduler's `nest_rs_database`) moved with
them; `current_container`, MCP's own, became crate-private, a locator no
operation needs beside `#[inject]`. **An item has one path**: its crate's, plus
`nest_rs::prelude`, the one prelude — a crate's own prelude
(`nest_rs::guards::prelude`) is a second path to every name it holds, and is
gone. What the docs, the CLI or a package hand out stays tier 1 whoever else
calls it, as `SchemaFn` did: the test is a developer's line, not the caller
count. A foreign library re-exported whole (`rmcp`, `sea_orm`,
`async_graphql`) keeps the paths `public-dependencies.md` gives it.

**2026-10-31 — the global pools' holders are gone, not kept as tier 2.** The
app-wide pools became declarations the root module lists
(`layer-contracts-below-the-edges.md`, 2026-10-31), so `GuardSpecs`,
`PipeSpecs` and `layer_chain::GlobalSpecs`, which the entry above moved to
`__private`, are deleted with the `use_*_global` seams that seeded them. The
tiers apply unchanged: `GlobalGuards`, `GlobalPipes`, `GlobalInterceptors`,
`GlobalExceptionFilters` and their list traits are a developer's line, tier 1;
what they expand to — core's `GlobalLayer`, `declare_global` and
`global_pool_declared` — is core's `__private`; the list traits' `__specs`
and the macro writing each family's tuple lists (`__global_layer_pool!`) are
what Rust cannot place in a module, so they keep a `__` prefix and stay
hidden. `layer_chain` stays public for `LayerSpec`.

**2026-10-31 — the layer specs a decorator builds are tier 2.** Once the
app-wide pools became declarations, no app wrote `guard::<G>()`, `pipe::<P>()`,
`interceptor::<I>()` or `exception_filter::<F>()`, nor named `GuardSpec`,
`PipeSpec`, `InterceptorSpec` or `ExceptionFilterSpec`: the `#[use_*]`
expansions, the four declarations and the edges' site composition do. Each
moved to its crate's `__private`, the paths the decorators emit with them,
as the pools' holders did above. `LayerSpec` stays tier 1 — `layer_chain` is
public for it, and a guard or pipe spec is one — and the attribute
`#[interceptor]` keeps `nest_rs::core::interceptor`, which named the function
too. The one compile-fail fixture calling `exception_filter` by hand now lists
the type in `GlobalExceptionFilters<(…)>`, where a developer meets the
refusal.

