//! Layer System — the unified vocabulary for cross-cutting concerns.
//!
//! A *layer* is any cross-cutting concern that wraps a handler. There are
//! four canonical [`LayerKind`]s:
//!
//! - [`LayerKind::Guard`] — gates access.
//! - [`LayerKind::Interceptor`] — wraps handler execution (logging, txn,
//!   response shaping, request preprocessing).
//! - [`LayerKind::Pipe`] — input transform / validation.
//! - [`LayerKind::ExceptionFilter`] — maps thrown errors to responses.
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
//! `nest_rs_filters`, `nest_rs_exception_filters` for the sub-traits — one
//! crate per [`LayerKind`].

use std::sync::Arc;

/// What kind of layer this is — one of the four canonical roles. Drives
/// the fixed execution order across kinds; intra-kind order comes from
/// declaration plus [`Layer::priority`].
///
/// Each sub-trait corresponds to exactly one variant — the kind is
/// determined by the trait, not by an instance method, so there is no
/// runtime ambiguity about what role a layer plays.
///
/// Pre-handler request shaping has no dedicated variant: it is expressed
/// as an [`Interceptor`](../../nest_rs_interceptors/trait.Interceptor.html).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LayerKind {
    /// Gates access.
    Guard,
    /// Wraps handler execution.
    Interceptor,
    /// Input transform / validation.
    Pipe,
    /// Maps thrown errors to responses.
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
pub enum LayerSite {
    /// `App::builder().use_*_global(...)`.
    Global,
    /// `#[module(layers = ...)]`.
    Module,
    /// `#[use_*]` on a controller/resolver/gateway struct.
    Controller,
    /// `#[use_*]` beside an individual handler/method.
    Method,
    /// Not bound to any explicit site.
    Inherited,
}

impl LayerSite {
    /// Lower number = broader. Used to pick the winner when the same
    /// [`TypeId`](std::any::TypeId) appears at several sites.
    pub fn broadness(self) -> u8 {
        match self {
            Self::Global => 0,
            Self::Module => 1,
            Self::Controller => 2,
            Self::Method => 3,
            Self::Inherited => 4,
        }
    }

    /// Lowercase label for dedup diagnostics and boot logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Module => "module",
            Self::Controller => "controller",
            Self::Method => "method",
            Self::Inherited => "inherited",
        }
    }
}

/// Common metadata for every layer kind. Sub-traits ([`Guard`](../../nest_rs_guards/trait.Guard.html),
/// [`Interceptor`](../../nest_rs_interceptors/trait.Interceptor.html),
/// [`Filter`](../../nest_rs_filters/trait.Filter.html),
/// [`GlobalPipe`](../../nest_rs_pipes/trait.GlobalPipe.html),
/// [`ExceptionFilter`](../../nest_rs_exception_filters/trait.ExceptionFilter.html))
/// extend this to pick up [`Layer::priority`] and a dedup-friendly identity.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_broadness_orders_global_to_method() {
        let mut sites = [
            LayerSite::Method,
            LayerSite::Global,
            LayerSite::Controller,
            LayerSite::Module,
        ];
        sites.sort_by_key(|s| s.broadness());
        assert_eq!(
            sites,
            [
                LayerSite::Global,
                LayerSite::Module,
                LayerSite::Controller,
                LayerSite::Method,
            ]
        );
    }
}
