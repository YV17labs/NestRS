//! Adds [`AppBuilderExceptionFiltersExt::use_exception_filters_global`] to
//! [`AppBuilder`].

use nest_rs_core::__private::check_specs_resolvable;
use nest_rs_core::AppBuilder;

use crate::registry::{ExceptionFilterSpec, ExceptionFilterSpecs};

/// Adds `.use_exception_filters_global(...)` to [`AppBuilder`].
///
/// The example on [`exception_filter`](fn@crate::exception_filter) registers through it.
pub trait AppBuilderExceptionFiltersExt: Sized {
    /// Register `specs` as the global exception-filter chain — the pool every
    /// **route** composes in, deduped by type against controller/method-scope
    /// declarations.
    ///
    /// Read per route by the `#[routes]` composer, so an error raised where no
    /// route matched (a 404, `/graphql`, `/mcp`, a WS upgrade) never reaches them.
    fn use_exception_filters_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = ExceptionFilterSpec>;
}

impl AppBuilderExceptionFiltersExt for AppBuilder {
    fn use_exception_filters_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = ExceptionFilterSpec>,
    {
        self.provide(ExceptionFilterSpecs(specs.into_iter().collect()))
            .provide_wiring("nest_rs::exception_filters::global", |container| {
                // At the wiring step, so an app serving no HTTP refuses it too.
                check_specs_resolvable::<ExceptionFilterSpecs>(
                    container,
                    "exception filter",
                    "an unresolvable global exception filter would silently drop its typed catch",
                )
                .map_err(nest_rs_core::anyhow::Error::msg)
            })
    }
}
