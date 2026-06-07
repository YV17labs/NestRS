//! Layer registration — typed specs the builder uses to seed the global
//! exception-filter chain into the container.

use std::any::{TypeId, type_name};
use std::sync::Arc;

use nest_rs_core::Container;

use crate::ExceptionFilter;
use crate::erased::ExceptionFilterErased;

/// One entry in the `use_exception_filters_global` list. Resolved against the
/// live container at configure time.
///
/// `type_id` identifies the filter *type* (used for dedup against
/// controller- and method-scope declarations), not the exception type.
pub struct ExceptionFilterSpec {
    pub type_id: TypeId,
    pub name: &'static str,
    pub(crate) resolve: fn(&Container) -> Option<Arc<dyn ExceptionFilterErased>>,
}

/// Construct an [`ExceptionFilterSpec`] for the given filter type.
///
/// ```rust,ignore
/// App::builder()
///     .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
///     .module::<AppModule>()
/// ```
pub fn exception_filter<F>() -> ExceptionFilterSpec
where
    F: ExceptionFilter + 'static,
{
    ExceptionFilterSpec {
        type_id: TypeId::of::<F>(),
        name: type_name::<F>(),
        resolve: |c| c.get::<F>().map(|arc| arc as Arc<dyn ExceptionFilterErased>),
    }
}

impl ExceptionFilterSpec {
    pub fn resolve(&self, container: &Container) -> Option<Arc<dyn ExceptionFilterErased>> {
        (self.resolve)(container)
    }
}

/// The unresolved `Vec<ExceptionFilterSpec>` seeded into the container by
/// `AppBuilder::use_exception_filters_global(...)`. The HTTP shaper reads it
/// at configure time and resolves against the live container.
pub struct ExceptionFilterSpecs(pub Vec<ExceptionFilterSpec>);

impl ExceptionFilterSpecs {
    pub fn type_ids(&self) -> Vec<(TypeId, &'static str)> {
        self.0.iter().map(|s| (s.type_id, s.name)).collect()
    }
}
