//! Layer registration — typed specs the builder uses to seed the global
//! interceptor chain into the container.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::LayerSpec;

use crate::interceptor::Interceptor;

/// One entry in the `use_interceptors_global` list. Resolved against the live
/// container at configure time.
pub type InterceptorSpec = LayerSpec<dyn Interceptor>;

/// Construct an [`InterceptorSpec`] for the given interceptor type.
///
/// ```
/// # use nest_rs_core::{App, Layer, injectable, module};
/// # use nest_rs_interceptors::{AppBuilderInterceptorsExt, Interceptor, Next, async_trait, interceptor};
/// # #[injectable]
/// # #[derive(Default)]
/// # struct ServerTiming;
/// # impl Layer for ServerTiming {}
/// # #[async_trait]
/// # impl Interceptor for ServerTiming {
/// #     async fn intercept(&self, req: poem::Request, next: Next<'_>) -> poem::Result<poem::Response> {
/// #         next.run(req).await
/// #     }
/// # }
/// # #[module(providers = [ServerTiming])]
/// # struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// App::builder()
///     .use_interceptors_global([interceptor::<ServerTiming>()])
///     .module::<AppModule>()
/// #   .build().await?;
/// # Ok(())
/// # }
/// ```
pub fn interceptor<I: Interceptor + 'static>() -> InterceptorSpec {
    LayerSpec::new(TypeId::of::<I>(), std::any::type_name::<I>(), |c| {
        c.get::<I>().map(|arc| arc as Arc<dyn Interceptor>)
    })
}

/// The unresolved `Vec<InterceptorSpec>` seeded into the container by
/// `AppBuilder::use_interceptors_global(...)`. The HTTP shaper reads it at
/// configure time and resolves against the live container.
pub struct InterceptorSpecs(pub Vec<InterceptorSpec>);

impl nest_rs_core::layer_chain::GlobalSpecs for InterceptorSpecs {
    type Layer = dyn Interceptor;
    fn specs(&self) -> &[InterceptorSpec] {
        &self.0
    }
}
