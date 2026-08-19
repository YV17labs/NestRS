//! Layer System — the unified vocabulary for cross-cutting concerns.
//!
//! A *layer* is any cross-cutting concern that wraps a handler. There are
//! five canonical [`LayerKind`]s, one per sub-trait crate:
//!
//! - [`LayerKind::Guard`] — gates access.
//! - [`LayerKind::Interceptor`] — wraps handler execution (logging, txn,
//!   response shaping, request preprocessing).
//! - [`LayerKind::Pipe`] — input transform / validation.
//! - [`LayerKind::Filter`] — maps an `Err` escaping the handler to a response.
//! - [`LayerKind::ExceptionFilter`] — maps a **typed** thrown error, closest to
//!   the handler.
//!
//! The execution order across kinds is fixed by the framework. On a routed
//! HTTP request: Guard → Pipe → scoped Interceptor → handler, with the
//! error path unwinding ExceptionFilter (typed catch, closest to the
//! handler) → Filter (generic mapper) → Interceptor (observer). Global
//! interceptors / filters execute at the transport edge instead — outside
//! routing — same relative nesting. Inside a single kind, the chain runs in
//! declaration order, with [`Layer::priority`] as an optional intra-kind
//! tiebreaker; priority orders entries *within* a site, never across sites.
//!
//! See `nest_rs_guards`, `nest_rs_pipes`, `nest_rs_interceptors`,
//! `nest_rs_filters`, `nest_rs_exception_filters` for the sub-traits — five
//! crates, and one [`LayerKind`] each.

use std::sync::Arc;

/// What kind of layer this is — one role per sub-trait, and the vocabulary the
/// fixed execution order across kinds is written in.
///
/// **Vocabulary, not state.** The framework constructs no value of this type and
/// matches on none: a layer's kind is decided by the sub-trait it implements, so
/// there is no `kind()` to override and nothing to keep in step at runtime. What
/// it is for is naming a slot in prose and in a doc link — `nest-rs-pipes` points
/// at [`Pipe`](Self::Pipe) to say where a global pipe runs.
///
/// **Five, and it shipped as four.** `Filter` had no variant while
/// `nest-rs-filters` shipped a `Layer` sub-trait like its four siblings, under a
/// module doc that reads "one crate per `LayerKind`" beside a list of five
/// crates. A vocabulary missing a member is worse than none: the reader counts
/// the list, finds four, and concludes the fifth family is something else.
///
/// Pre-handler request shaping has no dedicated variant: it is expressed as an
/// `Interceptor`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LayerKind {
    /// Gates access.
    Guard,
    /// Wraps handler execution.
    Interceptor,
    /// Input transform / validation.
    Pipe,
    /// Maps an `Err` escaping the handler to a response.
    Filter,
    /// Maps a **typed** thrown error to a response, closest to the handler.
    ExceptionFilter,
}

/// Where a layer was declared. Used by the dedup logic — when the same
/// [`TypeId`](std::any::TypeId) appears at several sites, the *broadest*
/// site wins because a wider declaration signals "this must run
/// everywhere — don't bypass it locally".
///
/// Named *Site* (not *Scope*) to disambiguate from request-scoped DI
/// resolution ([`RequestScope`](crate::RequestScope)). A Layer's site is
/// the place it was *declared*; it has nothing to do with the DI scope of
/// the Layer's provider.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LayerSite {
    /// `App::builder().use_*_global(...)`.
    Global,
    /// `#[use_*]` on the **host** struct — a controller, resolver, gateway or
    /// `#[mcp]` host.
    ///
    /// Named for the role every edge shares rather than for HTTP's word for it:
    /// this variant is what a guard declared on an `#[mcp]` host or a
    /// `#[resolver]` is reported under, and `controller` named a decorator
    /// their file does not contain. `framework.md` already calls the struct half
    /// of every pair the host.
    Host,
    /// `#[use_*]` beside an individual handler/method.
    Method,
}

impl LayerSite {
    /// Lowercase label for dedup diagnostics and boot logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Host => "host",
            Self::Method => "method",
        }
    }
}

/// Common metadata for every layer kind. Sub-traits — `Guard`, `Interceptor`,
/// `Filter`, `GlobalPipe`, `ExceptionFilter` — extend this to pick up
/// [`Layer::priority`] and a dedup-friendly identity.
///
/// Named rather than linked, and it is a limit rather than a preference:
/// rustdoc resolves an intra-doc link only against the dependency graph, and
/// this crate sits *below* all five, so it cannot have one. The hand-rolled
/// relative URLs that stood here (`../../nest_rs_guards/trait.Guard.html`)
/// resolved under a workspace-wide `cargo doc` and 404'd on docs.rs, where each
/// crate is published under its own root — a dead link on this crate's
/// most-read page.
///
/// The layer's [`LayerKind`] is determined by its sub-trait — there is no
/// `kind()` method to override.
pub trait Layer: Send + Sync + 'static {
    /// Tiebreaker inside a kind — lower runs first. Default `0`.
    /// Most layers should leave this at the default and rely on
    /// declaration order. Reach for a non-zero priority only when the
    /// framework's mechanical order doesn't capture a real dependency
    /// (e.g. a layer that must observe the request *before* every other
    /// layer of its kind regardless of how callers list it).
    fn priority(&self) -> i8 {
        0
    }

    /// Display name for boot logs and dedup diagnostics. Default = the
    /// implementor's type name (works for `Arc<dyn Layer>` via vtable
    /// monomorphisation per concrete impl).
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

impl<T: Layer + ?Sized> Layer for Arc<T> {
    fn priority(&self) -> i8 {
        (**self).priority()
    }

    fn name(&self) -> &'static str {
        (**self).name()
    }
}
