//! Adds [`AppBuilderFiltersExt::use_filters_global`] to
//! [`AppBuilder`](nest_rs_core::AppBuilder).

use nest_rs_core::AppBuilder;
use nest_rs_http::{HttpEndpointWrap, endpoint_wrap_priority};
use poem::endpoint::BoxEndpoint;
use poem::{EndpointExt, Response};

use crate::ext::FilterExt;
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
/// Like the other `use_*_global` extensions, this seeds two things at
/// once:
///
/// 1. [`FilterSpecs`] into the container — the per-route shaper reads
///    them for TypeId-based dedup against controller / method
///    declarations.
/// 2. An [`HttpEndpointWrap`] wrap that resolves the specs at HTTP
///    `configure` time and folds every global filter around the
///    assembled endpoint — so they fire on the error path of every
///    endpoint the HTTP transport mounts, including self-mounting
///    routes like `/graphql`, MCP, and WS upgrade.
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
            .provide_meta(HttpEndpointWrap::with_priority(
                endpoint_wrap_priority::FILTERS,
                |container, mut endpoint: BoxEndpoint<'static, Response>| {
                    let Some(specs) = container.get::<FilterSpecs>() else {
                        return endpoint;
                    };
                    // Wrap in declaration order — innermost first — so the
                    // first listed filter is the outermost mapper on the
                    // error path.
                    for spec in specs.0.iter().rev() {
                        if let Some(filter) = spec.resolve(container) {
                            endpoint = FilterExt::filter(endpoint, filter)
                                .map_to_response()
                                .boxed();
                        } else {
                            tracing::warn!(
                                target: "nest_rs::layers",
                                layer = spec.name,
                                "global filter not registered — skipping at runtime (boot-time access-graph validation should have caught this)",
                            );
                        }
                    }
                    endpoint
                },
            ))
    }
}
