//! Layer registration — typed specs the builder uses to seed the global
//! exception-filter chain into the container.
//!
//! `ExceptionFilterSpec` is a [`LayerSpec`](nest_rs_core::LayerSpec) alias — the
//! shared shape and its `resolve` method live in `nest-rs-core`; only the typed
//! constructor and the erased trait differ per family.

use std::any::{TypeId, type_name};
use std::sync::Arc;

use nest_rs_core::LayerSpec;

use crate::ExceptionFilter;
use crate::erased::ExceptionFilterErased;

/// One entry in the `use_exception_filters_global` list. Resolved against the
/// live container at configure time.
///
/// `type_id` identifies the filter *type* (used for dedup against
/// controller- and method-scope declarations), not the exception type.
pub type ExceptionFilterSpec = LayerSpec<dyn ExceptionFilterErased>;

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
    LayerSpec::new(TypeId::of::<F>(), type_name::<F>(), |c| {
        c.get::<F>()
            .map(|arc| arc as Arc<dyn ExceptionFilterErased>)
    })
}

/// The unresolved `Vec<ExceptionFilterSpec>` seeded into the container by
/// `AppBuilder::use_exception_filters_global(...)`. The HTTP shaper reads it
/// at configure time and resolves against the live container.
pub struct ExceptionFilterSpecs(pub Vec<ExceptionFilterSpec>);
