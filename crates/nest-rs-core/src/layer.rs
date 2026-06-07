//! Layer System — the unified vocabulary for cross-cutting concerns.
//!
//! A *layer* is any cross-cutting concern that wraps a handler. The five
//! canonical [`LayerKind`]s map 1:1 to NestJS:
//!
//! - [`LayerKind::Middleware`] — general request pre-/post-processing.
//! - [`LayerKind::Guard`] — gates access (the `CanActivate` analog).
//! - [`LayerKind::Interceptor`] — wraps handler execution (logging, txn,
//!   response shaping).
//! - [`LayerKind::Pipe`] — input transform / validation.
//! - [`LayerKind::ExceptionFilter`] — maps thrown errors to responses.
//!
//! The execution order across kinds is fixed by the framework (Middleware →
//! Guard → Interceptor → Pipe → handler → Interceptor (post) → Exception
//! Filter on error). Inside a single kind, the chain runs in declaration
//! order, with [`Layer::priority`] as an optional intra-kind tiebreaker.
//!
//! ## `#[public]` is metadata, not framework magic
//!
//! `#[public]` attaches a [`Public`] marker to the route via the existing
//! metadata mechanism — it does **not** cause the framework to skip any
//! guard. Each guard decides what "public" means for it: an `AbilityGuard`
//! may still run on a public route to apply visitor rules; an `AuthGuard`
//! may skip rejection when no token is present. The guard reads the marker
//! through the transport's reflector helper.
//!
//! See `nest_rs_guards`, `nest_rs_pipes`, `nest_rs_middleware` for the
//! sub-traits.

use std::sync::Arc;

/// What kind of layer this is — one of the five NestJS-canonical roles.
/// Drives the fixed execution order across kinds; intra-kind order comes
/// from declaration plus [`Layer::priority`].
///
/// Each sub-trait corresponds to exactly one variant — the kind is
/// determined by the trait, not by an instance method, so there is no
/// runtime ambiguity about what role a layer plays.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LayerKind {
    /// General request preprocessing/postprocessing.
    Middleware,
    /// Gates access — the `CanActivate` analog.
    Guard,
    /// Wraps handler execution.
    Interceptor,
    /// Input transform / validation.
    Pipe,
    /// Maps thrown errors to responses.
    ExceptionFilter,
}

/// Marker attached as request data when a handler is `#[public]`. The
/// framework does **not** act on it — guards read it via the transport's
/// reflector and decide whether to honor it.
///
/// ```rust,ignore
/// // In a guard:
/// if Reflector::new(req).is_public() {
///     // policy for public routes
/// }
/// ```
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Public;

/// Where a layer was declared. Used by the dedup logic — when the same
/// [`TypeId`](std::any::TypeId) appears at several scopes, the *broadest*
/// scope wins because a wider declaration signals "this must run
/// everywhere — don't bypass it locally".
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LayerScope {
    /// `App::builder().use_*_global(...)`.
    Global,
    /// `#[module(layers = ...)]`.
    Module,
    /// `#[use_*]` on a controller/resolver/gateway struct.
    Controller,
    /// `#[use_*]` beside an individual handler/method.
    Method,
    /// Not bound to any explicit scope.
    Inherited,
}

impl LayerScope {
    /// Lower number = broader. Used to pick the winner when the same
    /// [`TypeId`](std::any::TypeId) appears at several scopes.
    pub fn broadness(self) -> u8 {
        match self {
            Self::Global => 0,
            Self::Module => 1,
            Self::Controller => 2,
            Self::Method => 3,
            Self::Inherited => 4,
        }
    }
}

/// Common metadata for every layer kind. Sub-traits ([`Guard`](../../nest_rs_guards/trait.Guard.html),
/// [`Interceptor`](../../nest_rs_middleware/trait.Interceptor.html),
/// [`Filter`](../../nest_rs_middleware/trait.Filter.html),
/// [`GlobalPipe`](../../nest_rs_pipes/trait.GlobalPipe.html)) extend this to
/// pick up [`Layer::priority`] and a dedup-friendly identity.
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
    fn scope_broadness_orders_global_to_method() {
        let mut scopes = [
            LayerScope::Method,
            LayerScope::Global,
            LayerScope::Controller,
            LayerScope::Module,
        ];
        scopes.sort_by_key(|s| s.broadness());
        assert_eq!(
            scopes,
            [
                LayerScope::Global,
                LayerScope::Module,
                LayerScope::Controller,
                LayerScope::Method,
            ]
        );
    }
}
