//! Layers — the vocabulary of the cross-cutting concerns that wrap a unit.
//!
//! A *layer* is any cross-cutting concern that wraps the unit an edge
//! dispatches. There are four [`LayerKind`]s:
//!
//! - [`LayerKind::Guard`] — gates access (`nest_rs_guards`).
//! - [`LayerKind::Pipe`] — input transform / validation (`nest_rs_pipes`).
//! - [`LayerKind::Interceptor`] — wraps the handler: logging, a transaction,
//!   response shaping ([`Interceptor`](crate::Interceptor)).
//! - [`LayerKind::ExceptionFilter`] — answers a failure by its type
//!   ([`ExceptionFilter`](crate::ExceptionFilter)).
//!
//! Every edge runs them through one pipeline, whose stage order this crate
//! writes once, in the one function every edge calls: the unit stage, the
//! guard stage and the posture's class gate, the exception filters, the
//! handler stage, then the tail — the pipes and the call. Inside a stage the
//! sites run from the broadest in, each in declaration order, with
//! [`Layer::priority`] as the tiebreaker within a site — never across.

use std::sync::Arc;

use crate::operation_log::Unit;

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
    /// Answers a failure by its type, closest to the handler.
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

/// Common metadata for every layer kind. Sub-traits — `Guard`, `GlobalPipe`,
/// [`Interceptor`](crate::Interceptor), [`ExceptionFilter`](crate::ExceptionFilter)
/// — extend this to pick up [`Layer::priority`] and a dedup-friendly identity.
///
/// `Guard` and `GlobalPipe` are named, not linked: this crate sits below
/// theirs, and a relative URL 404s on docs.rs.
pub trait Layer: Send + Sync + 'static {
    /// Tiebreaker inside one site of a kind — lower runs first, never ahead of
    /// a broader site's layers. Default `0`, leaving declaration order in
    /// charge.
    fn priority(&self) -> i8 {
        0
    }

    /// Display name for boot logs and dedup diagnostics; defaults to the
    /// implementor's type name.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Whether this layer has work at `unit`. Read once when an app-wide list
    /// is composed for a site, never per call, so a layer serving one edge
    /// costs the others nothing; a layer a decorator binds at a site always
    /// runs there.
    fn serves(&self, unit: Unit) -> bool {
        let _ = unit;
        true
    }
}

impl<T: Layer + ?Sized> Layer for Arc<T> {
    fn priority(&self) -> i8 {
        (**self).priority()
    }

    fn name(&self) -> &'static str {
        (**self).name()
    }

    fn serves(&self, unit: Unit) -> bool {
        (**self).serves(unit)
    }
}
