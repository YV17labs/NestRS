//! Layer registration — typed specs the builder uses to seed the global
//! interceptor chain into the container.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::LayerSpec;

use crate::interceptor::Interceptor;

/// One interceptor, resolved against the container.
pub type InterceptorSpec = LayerSpec<dyn Interceptor>;

/// Construct an [`InterceptorSpec`] for the given interceptor type.
pub fn interceptor<I: Interceptor + 'static>() -> InterceptorSpec {
    LayerSpec::new(TypeId::of::<I>(), std::any::type_name::<I>(), |c| {
        c.get::<I>().map(|arc| arc as Arc<dyn Interceptor>)
    })
}
