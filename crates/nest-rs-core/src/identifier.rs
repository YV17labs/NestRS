//! The framework's identifier contract, and the one sentence a client reads when
//! a value breaks it.
//!
//! An id is a UUID **v7** here — sortable by construction, so a primary key
//! doubles as a creation order and a trace id can be one — and three unrelated
//! sites enforce it: the gate `#[crud]` generates on a path id, `Bind<S, A>`'s
//! HTTP extractor, and the GraphQL `bind` helper.
//!
//! **Declared in the kernel because it is the only crate all three reach.** It
//! lived in `nest-rs-codegen`, which is a macro-time helper: a runtime crate
//! cannot depend on it (it pulls `syn`), so two of the three sites spelled the
//! literal instead — one of them with a comment conceding the point. The
//! wording had already drifted once (`"path id must be a UUID v7"` against
//! `"id must be a UUID v7"`), and nothing would have said so a second time.
//! `#[crud]` emits the **path** rather than interpolating the value, which is
//! what removes the macro-time dependency the old placement was built around.

/// What a client is told when an id is not a UUID v7.
///
/// Shared so a caller cannot tell from the wording which of the three sites
/// refused it, and so a change to that wording cannot land on one and miss the
/// other two.
pub const UUID_V7_REQUIRED: &str = "id must be a UUID v7";
