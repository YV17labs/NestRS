//! Adds [`AppBuilderInterceptorsExt::use_interceptors_global`] to
//! [`AppBuilder`](nest_rs_core::AppBuilder).

use nest_rs_core::AppBuilder;

use crate::registry::{InterceptorSpec, InterceptorSpecs};

/// Adds `.use_interceptors_global(...)` to [`AppBuilder`].
///
/// ```rust,ignore
/// use nest_rs_interceptors::{AppBuilderInterceptorsExt, interceptor};
///
/// App::builder()
///     .use_interceptors_global([interceptor::<ServerTiming>()])
///     .module::<AppModule>()
///     .build().await?
///     .run().await
/// ```
///
/// Declaration order matters: the runtime chain wraps the handler in the
/// reverse order of declaration (first listed = outermost), with
/// [`Layer::priority`](nest_rs_core::Layer::priority) as an optional
/// tiebreaker.
pub trait AppBuilderInterceptorsExt: Sized {
    fn use_interceptors_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = InterceptorSpec>;
}

impl AppBuilderInterceptorsExt for AppBuilder {
    fn use_interceptors_global<I>(self, specs: I) -> Self
    where
        I: IntoIterator<Item = InterceptorSpec>,
    {
        self.provide(InterceptorSpecs(specs.into_iter().collect()))
    }
}
