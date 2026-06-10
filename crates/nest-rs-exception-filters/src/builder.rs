//! Adds [`AppBuilderExceptionFiltersExt::use_exception_filters_global`] to
//! [`AppBuilder`](nest_rs_core::AppBuilder).

use nest_rs_core::AppBuilder;
use nest_rs_http::HttpBootCheck;

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
            .provide_meta(HttpBootCheck::new(|container| {
                let Some(specs) = container.get::<ExceptionFilterSpecs>() else {
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
                        "global exception filter(s) not resolvable from the container: {} — \
                         import the module that provides them; an unresolvable global \
                         exception filter would silently drop its typed catch",
                        missing.join(", "),
                    ))
                }
            }))
    }
}
