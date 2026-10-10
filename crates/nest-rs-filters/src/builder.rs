//! Adds [`AppBuilderFiltersExt::use_filters_global`] to
//! [`AppBuilder`].

use nest_rs_core::__private::{
    ResolvedLayer, check_specs_resolvable, compose_chain, resolve_global_layers,
};
use nest_rs_core::{AppBuilder, Container};
use nest_rs_http::__private::{HttpEndpointWrap, endpoint_wrap_priority};
use poem::EndpointExt;

use crate::filter::{Filter, FilterChain};
use crate::registry::{FilterSpec, FilterSpecs};

/// Adds `.use_filters_global(...)` to [`AppBuilder`].
///
/// The example on [`filter`](fn@crate::filter) registers through it.
///
/// The chain wraps the whole routing tree at the transport edge (the filters'
/// band), mapping every error escaping routing; it sits outside the ambient DB
/// context, so a failed transaction has already rolled back when it maps.
pub trait AppBuilderFiltersExt: Sized {
    /// Register `specs` as the global filter chain — the transport-edge pool
    /// that maps every error escaping routing, deduped by type against
    /// controller/method scope (broadest wins).
    fn use_filters_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = FilterSpec>;
}

impl AppBuilderFiltersExt for AppBuilder {
    fn use_filters_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = FilterSpec>,
    {
        let collected: Vec<FilterSpec> = specs.into_iter().collect();
        self.provide(FilterSpecs(collected))
            .provide_wiring(GLOBAL_POOL, check_global_pool)
            .provide_meta(HttpEndpointWrap::with_priority(
                endpoint_wrap_priority::FILTERS,
                |container, endpoint| {
                    let chain = global_chain(container);
                    if chain.is_empty() {
                        return endpoint;
                    }
                    FilterChain::new(endpoint, chain).boxed()
                },
            ))
    }
}

/// The global filter pool, as a boot failure names its check.
const GLOBAL_POOL: &str = "nest_rs::filters::global";

/// Refuse a global filter no imported module provides, in every app — one that
/// serves no HTTP included — before any hook runs.
fn check_global_pool(container: &Container) -> nest_rs_core::anyhow::Result<()> {
    let Some(specs) = container.get::<FilterSpecs>() else {
        return Ok(());
    };
    check_specs_resolvable(
        &specs.0,
        container,
        "filter",
        "an unresolvable global filter would silently drop its error mapping",
    )
    .map_err(nest_rs_core::anyhow::Error::msg)
}

/// Resolve `FilterSpecs` into the deduplicated, priority-ordered global chain.
fn global_chain(container: &Container) -> Vec<ResolvedLayer<dyn Filter>> {
    let global = resolve_global_layers::<FilterSpecs>(container);
    compose_chain::<dyn Filter>(global, Vec::new(), Vec::new(), &[], "transport")
}
