//! Adds [`AppBuilderFiltersExt::use_filters_global`] to
//! [`AppBuilder`].

use nest_rs_core::layer_chain::{ResolvedLayer, compose_chain, resolve_global_layers};
use nest_rs_core::{AppBuilder, Container, check_specs_resolvable};
use nest_rs_http::{HttpBootCheck, HttpEndpointWrap, endpoint_wrap_priority};
use poem::EndpointExt;

use crate::filter::{Filter, FilterChain};
use crate::registry::{FilterSpec, FilterSpecs};

/// Adds `.use_filters_global(...)` to [`AppBuilder`].
///
/// The example on [`filter`](fn@crate::filter) registers through it.
///
/// The chain wraps the whole routing tree at the transport edge (band
/// [`FILTERS`](nest_rs_http::endpoint_wrap_priority::FILTERS)), mapping every error
/// escaping routing; it sits outside the ambient DB context, so a failed
/// transaction has already rolled back when it maps.
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
            .provide_meta(HttpBootCheck::new(|container| {
                let Some(specs) = container.get::<FilterSpecs>() else {
                    return Ok(());
                };
                check_specs_resolvable(
                    &specs.0,
                    container,
                    "filter",
                    "an unresolvable global filter would silently drop its error mapping",
                )
            }))
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

/// Resolve `FilterSpecs` into the deduplicated, priority-ordered global chain.
fn global_chain(container: &Container) -> Vec<ResolvedLayer<dyn Filter>> {
    let global = resolve_global_layers::<FilterSpecs>(container);
    compose_chain::<dyn Filter>(global, Vec::new(), Vec::new(), &[], "transport")
}
