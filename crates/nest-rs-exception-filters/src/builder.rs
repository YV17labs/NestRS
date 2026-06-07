//! Adds [`AppBuilderExceptionFiltersExt::use_exception_filters_global`] to
//! [`AppBuilder`](nest_rs_core::AppBuilder).

use nest_rs_core::AppBuilder;

use crate::registry::{ExceptionFilterSpec, ExceptionFilterSpecs};

/// Adds `.use_exception_filters_global(...)` to [`AppBuilder`].
///
/// ```rust,ignore
/// use nest_rs_exception_filters::{AppBuilderExceptionFiltersExt, exception_filter};
///
/// App::builder()
///     .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
///     .module::<AppModule>()
///     .build().await?
///     .run().await
/// ```
pub trait AppBuilderExceptionFiltersExt: Sized {
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
    }
}
