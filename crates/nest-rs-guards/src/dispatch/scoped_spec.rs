//! Scoped layer specs — the macro-emitted form of a controller- /
//! resolver- / gateway- / method-scope layer declaration. Carries the
//! `TypeId` so dedup against the global chain finds the same key.

use nest_rs_core::Container;
use nest_rs_exception_filters::ExceptionFilterErased;
use nest_rs_filters::Filter;
use nest_rs_interceptors::Interceptor;
use nest_rs_pipes::GlobalPipe;

use nest_rs_core::layer_chain::{LayerSite, LayerSpec, ResolvedLayer, resolve_global_layers};

use crate::Guard;

/// A scoped layer spec (controller / resolver / gateway / handler) — the same
/// shape a global registration carries, only tagged with a narrower
/// [`LayerSite`] when it is resolved, so dedup against the global chain finds
/// the same `TypeId` key. It **is** [`LayerSpec`]: one type, one constructor,
/// no second structure to keep in step.
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

pub(crate) fn resolve_specs<L: ?Sized>(
    container: &Container,
    specs: &[ScopedLayerSpec<L>],
    source: LayerSite,
) -> Vec<ResolvedLayer<L>> {
    specs
        .iter()
        .filter_map(|spec| {
            spec.resolve(container).map(|layer| ResolvedLayer {
                type_id: spec.type_id,
                name: spec.name,
                source,
                layer,
            })
        })
        .collect()
}
