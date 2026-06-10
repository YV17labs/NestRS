//! Adds [`AppBuilderFiltersExt::use_filters_global`] to
//! [`AppBuilder`](nest_rs_core::AppBuilder).

use nest_rs_core::layer_chain::{LayerSite, ResolvedLayer, compose_chain};
use nest_rs_core::{AppBuilder, Container};
use nest_rs_http::{HttpBootCheck, HttpEndpointWrap, endpoint_wrap_priority};
use poem::EndpointExt;

use crate::filter::{Filter, FilterEndpoint};
use crate::registry::{FilterSpec, FilterSpecs};

/// Adds `.use_filters_global(...)` to [`AppBuilder`].
///
/// ```rust,ignore
/// use nest_rs_filters::{AppBuilderFiltersExt, filter};
///
/// App::builder()
///     .use_filters_global([filter::<ProblemDetailsFilter>()])
///     .module::<AppModule>()
///     .build().await?
///     .run().await
/// ```
///
/// This seeds [`FilterSpecs`] into the container and attaches the
/// transport-edge wrap that executes the **global** sub-chain around the
/// whole routing tree (band
/// [`FILTERS`](nest_rs_http::endpoint_wrap_priority::FILTERS)): a global
/// filter maps every error escaping routing — handler errors no narrower
/// filter claimed, 404s, self-mount errors. It sits *outside* the ambient
/// DB context, so the failed transaction has already rolled back by the
/// time it maps; a global filter can never turn a rollback into a commit.
/// The per-route composer dedups controller / method redeclarations against
/// this global scope by `TypeId` (broadest wins), so any filter still
/// executes exactly once.
pub trait AppBuilderFiltersExt: Sized {
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
                let missing: Vec<&str> = specs
                    .0
                    .iter()
                    .filter(|s| s.resolve(container).is_none())
                    .map(|s| s.name)
                    .collect();
                if missing.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "global filter(s) not resolvable from the container: {} — import the \
                         module that provides them; an unresolvable global filter would \
                         silently drop its error mapping",
                        missing.join(", "),
                    ))
                }
            }))
            .provide_meta(HttpEndpointWrap::with_priority(
                endpoint_wrap_priority::FILTERS,
                |container, endpoint| {
                    let chain = global_chain(container);
                    // First declared = outermost on the error path.
                    let mut ep = endpoint;
                    for entry in chain.into_iter().rev() {
                        ep = FilterEndpoint::new(ep, entry.layer)
                            .map_to_response()
                            .boxed();
                    }
                    ep
                },
            ))
    }
}

/// Resolve `FilterSpecs` into the deduplicated, priority-ordered global
/// chain — same `compose_chain` as every other Layer System site.
fn global_chain(container: &Container) -> Vec<ResolvedLayer<dyn Filter>> {
    let mut global: Vec<ResolvedLayer<dyn Filter>> = Vec::new();
    if let Some(specs) = container.get::<FilterSpecs>() {
        for spec in &specs.0 {
            if let Some(layer) = spec.resolve(container) {
                global.push(ResolvedLayer {
                    type_id: spec.type_id,
                    name: spec.name,
                    source: LayerSite::Global,
                    layer,
                });
            }
        }
    }
    compose_chain::<dyn Filter>(global, Vec::new(), Vec::new(), &[], "transport")
}
