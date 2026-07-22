//! Layer registration — typed specs the builder uses to seed the global
//! interceptor chain into the container.
//!
//! `InterceptorSpec` is a [`LayerSpec`](nest_rs_core::LayerSpec) alias — the
//! shared shape and its `resolve` method live in `nest-rs-core`; only the typed
//! constructor and the erased trait differ per family.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::LayerSpec;

use crate::interceptor::Interceptor;

/// One entry in the `use_interceptors_global` list. Resolved against the live
/// container at configure time.
pub type InterceptorSpec = LayerSpec<dyn Interceptor>;

/// Construct an [`InterceptorSpec`] for the given interceptor type.
///
/// ```rust,ignore
/// App::builder()
///     .use_interceptors_global([interceptor::<ServerTiming>()])
///     .module::<AppModule>()
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
