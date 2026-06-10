//! Scoped layer specs — the macro-emitted form of a controller- /
//! resolver- / gateway- / method-scope layer declaration. Carries the
//! `TypeId` so dedup against the global chain finds the same key.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_exception_filters::ExceptionFilterErased;
use nest_rs_filters::Filter;
use nest_rs_interceptors::Interceptor;
use nest_rs_pipes::GlobalPipe;

use nest_rs_core::layer_chain::{LayerSite, ResolvedLayer};

use crate::Guard;

/// A scoped layer spec (controller / resolver / gateway / handler).
/// Carries the `TypeId` so dedup against the global chain finds the same
/// key, plus a `resolve` fn pointer that recovers the concrete `Arc<L>`
/// from the container at first request.
pub struct ScopedLayerSpec<L: ?Sized> {
    pub type_id: TypeId,
    pub name: &'static str,
    pub resolve: fn(&Container) -> Option<Arc<L>>,
}

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

pub(crate) fn resolve_specs<L: ?Sized>(
    container: &Container,
    specs: &[ScopedLayerSpec<L>],
    source: LayerSite,
) -> Vec<ResolvedLayer<L>> {
    specs
        .iter()
        .filter_map(|spec| {
            (spec.resolve)(container).map(|layer| ResolvedLayer {
                type_id: spec.type_id,
                name: spec.name,
                source,
                layer,
            })
        })
        .collect()
}
