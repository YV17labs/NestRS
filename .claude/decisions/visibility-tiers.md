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
