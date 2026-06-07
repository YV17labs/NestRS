//! Layer registration — typed specs the builder uses to seed the global
//! interceptor chain into the container.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::Container;

use crate::interceptor::Interceptor;

/// One entry in the `use_interceptors_global` list. Resolved against the live
/// container at configure time.
pub struct InterceptorSpec {
    pub type_id: TypeId,
    pub name: &'static str,
    pub(crate) resolve: fn(&Container) -> Option<Arc<dyn Interceptor>>,
}

/// Construct an [`InterceptorSpec`] for the given interceptor type.
///
/// ```rust,ignore
/// App::builder()
///     .use_interceptors_global([interceptor::<ServerTiming>()])
///     .module::<AppModule>()
/// ```
pub fn interceptor<I: Interceptor + 'static>() -> InterceptorSpec {
    InterceptorSpec {
        type_id: TypeId::of::<I>(),
        name: std::any::type_name::<I>(),
        resolve: |c| c.get::<I>().map(|arc| arc as Arc<dyn Interceptor>),
    }
}

impl InterceptorSpec {
    pub fn resolve(&self, container: &Container) -> Option<Arc<dyn Interceptor>> {
        (self.resolve)(container)
    }
}

/// The unresolved `Vec<InterceptorSpec>` seeded into the container by
/// `AppBuilder::use_interceptors_global(...)`. The HTTP shaper reads it at
/// configure time and resolves against the live container.
pub struct InterceptorSpecs(pub Vec<InterceptorSpec>);

impl InterceptorSpecs {
    /// Type-ids paired with names, for dedup queries from the per-route shaper.
    pub fn type_ids(&self) -> Vec<(TypeId, &'static str)> {
        self.0.iter().map(|s| (s.type_id, s.name)).collect()
    }
}
