//! Adds [`AppBuilderFiltersExt::use_filters_global`] to
//! [`AppBuilder`](nest_rs_core::AppBuilder).

use nest_rs_core::AppBuilder;

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
        self.provide(FilterSpecs(specs.into_iter().collect()))
    }
}
