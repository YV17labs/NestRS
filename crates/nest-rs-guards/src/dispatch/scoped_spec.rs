//! Scoped layer specs — the macro-emitted form of a controller- /
//! resolver- / gateway- / method-scope layer declaration. Carries the
//! `TypeId` so dedup against the global chain finds the same key.

use nest_rs_core::{Container, UnresolvedLayerError};
use nest_rs_exception_filters::ExceptionFilterErased;
use nest_rs_filters::Filter;
use nest_rs_interceptors::Interceptor;
use nest_rs_pipes::GlobalPipe;

use nest_rs_core::__private::{ResolvedLayer, refuse_site, resolve_global_layers};
use nest_rs_core::layer_chain::{LayerSite, LayerSpec};

use crate::Guard;

/// A scoped layer spec (controller / resolver / gateway / handler): a
/// [`LayerSpec`] tagged with a narrower [`LayerSite`] when it is resolved, so
/// dedup against the global chain finds the same `TypeId` key.
pub type ScopedLayerSpec<L> = LayerSpec<L>;

/// A guard spec for a specific scope.
pub type ScopedGuardSpec = ScopedLayerSpec<dyn Guard>;
/// A pipe spec for a specific scope — used when the route or controller
/// declares `#[use_pipes(...)]` (rare; most pipes are global).
pub type ScopedPipeSpec = ScopedLayerSpec<dyn GlobalPipe>;
/// An exception-filter spec for a specific scope — used when the route
/// or controller declares `#[use_exception_filters(...)]`.
pub type ScopedExceptionFilterSpec = ScopedLayerSpec<dyn ExceptionFilterErased>;
/// An interceptor spec for a specific scope — used when the route or
/// controller declares `#[use_interceptors(...)]`.
pub type ScopedInterceptorSpec = ScopedLayerSpec<dyn Interceptor>;
/// A filter spec for a specific scope — used when the route or controller
/// declares `#[use_filters(...)]`.
pub type ScopedFilterSpec = ScopedLayerSpec<dyn Filter>;

/// Resolve the global guard pool from the container into `LayerSite::Global`
/// entries — the single implementation the route shaper and the boot-time
/// chain validation both compose from, so their dedup inputs cannot drift.
pub(crate) fn resolve_global_guards(container: &Container) -> Vec<ResolvedLayer<dyn crate::Guard>> {
    resolve_global_layers::<crate::registry::GuardSpecs>(container)
}

/// Resolve one site's scoped declarations into entries tagged `scope`, so dedup
/// against the global chain finds the same key.
///
/// A spec whose provider no imported module registers refuses, naming the layer
/// and `site` (as its boot log names it): a site never runs without a layer it
/// declares.
pub(crate) fn resolve_scoped<L: ?Sized>(
    container: &Container,
    specs: &[ScopedLayerSpec<L>],
    scope: LayerSite,
    site: &str,
) -> Result<Vec<ResolvedLayer<L>>, UnresolvedLayerError> {
    specs
        .iter()
        .map(|spec| match spec.resolve(container) {
            Some(layer) => Ok(ResolvedLayer {
                type_id: spec.type_id,
                name: spec.name,
                source: scope,
                layer,
            }),
            None => Err(UnresolvedLayerError {
                layer: spec.name,
                site: site.to_owned(),
            }),
        })
        .collect()
}

/// The one `error` a site files when a layer it declares does not resolve —
/// for the operator; the client reads an opaque refusal.
pub(crate) fn report_unresolved(unresolved: &UnresolvedLayerError) {
    tracing::error!(
        target: nest_rs_core::target::LAYERS,
        layer = unresolved.layer,
        site = unresolved.site.as_str(),
        "a layer the site declares is provided by no imported module",
    );
}

/// Refuse a site composed at mount whose declared layer does not resolve: its
/// `error`, and the refusal filed on `container` for the boot to fail on.
pub(crate) fn refuse_unresolved(container: &Container, unresolved: UnresolvedLayerError) {
    report_unresolved(&unresolved);
    refuse_site(container, unresolved.into());
}
