//! Adds [`AppBuilderExceptionFiltersExt::use_exception_filters_global`] to
//! [`AppBuilder`](nest_rs_core::AppBuilder).

use nest_rs_core::{AppBuilder, check_specs_resolvable};
use nest_rs_http::HttpBootCheck;

use crate::registry::{ExceptionFilterSpec, ExceptionFilterSpecs};

/// Adds `.use_exception_filters_global(...)` to [`AppBuilder`].
///
/// ```rust,ignore
/// use nest_rs::prelude::App;
/// use nest_rs::exception_filters::{AppBuilderExceptionFiltersExt, exception_filter};
///
/// App::builder()
///     .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
///     .module::<AppModule>()
///     .build().await?
///     .run().await
/// ```
pub trait AppBuilderExceptionFiltersExt: Sized {
    /// Register `specs` as the global exception-filter chain — the pool every
    /// **route** composes in, deduped by type against controller/method-scope
    /// declarations.
    ///
    /// Scope note: unlike a global `Filter` (`nest_rs_filters`), which attaches
    /// a wrap at the transport edge, these are read per route by the `#[routes]`
    /// composer. An error raised where no route matched — a 404, a self-mounted
    /// surface such as `/graphql` or `/mcp`, a WS upgrade — never reaches them.
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
            .provide_meta(HttpBootCheck::new(|container| {
                let Some(specs) = container.get::<ExceptionFilterSpecs>() else {
                    return Ok(());
                };
                check_specs_resolvable(
                    &specs.0,
                    container,
                    "exception filter",
                    "an unresolvable global exception filter would silently drop its typed catch",
                )
            }))
    }
}
