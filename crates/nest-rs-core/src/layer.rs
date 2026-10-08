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
//! declaration order, with [`Layer::priority`] as the tiebreaker within a site.
//!
//! The sub-traits live in `nest_rs_guards`, `nest_rs_pipes`,
//! `nest_rs_interceptors`, `nest_rs_filters` and `nest_rs_exception_filters`.

use std::sync::Arc;

/// What kind of layer this is — one role per sub-trait, and the vocabulary the
/// fixed execution order across kinds is written in.
///
/// Vocabulary, not state: a layer's kind is decided by the sub-trait it
/// implements. Pre-handler request shaping is an `Interceptor`.
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

/// Where a layer was declared. When the same [`TypeId`](std::any::TypeId)
/// appears at several sites, the broadest one wins.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LayerSite {
    /// `App::builder().use_*_global(...)`.
    Global,
    /// `#[use_*]` on the **host** struct — a controller, resolver, gateway or
    /// `#[mcp]` host.
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
/// The sub-traits are named, not linked: this crate sits below theirs, and a
/// relative URL 404s on docs.rs.
pub trait Layer: Send + Sync + 'static {
    /// Tiebreaker inside a kind — lower runs first. Default `0`, leaving
    /// declaration order in charge.
    fn priority(&self) -> i8 {
        0
    }

    /// Display name for boot logs and dedup diagnostics; defaults to the
    /// implementor's type name.
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
