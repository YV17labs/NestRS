//! Layer registration — typed specs the builder uses to seed the global
//! exception-filter chain into the container.

use std::any::{TypeId, type_name};
use std::sync::Arc;

use nest_rs_core::LayerSpec;

use crate::ExceptionFilter;
use crate::erased::ExceptionFilterErased;

/// One exception filter, resolved against the container.
///
/// `type_id` identifies the filter *type* (used for dedup against
/// controller- and method-scope declarations), not the exception type.
pub type ExceptionFilterSpec = LayerSpec<dyn ExceptionFilterErased>;

/// Construct an [`ExceptionFilterSpec`] for the given filter type.
pub fn exception_filter<F>() -> ExceptionFilterSpec
where
    F: ExceptionFilter + 'static,
{
    LayerSpec::new(TypeId::of::<F>(), type_name::<F>(), |c| {
        c.get::<F>()
            .map(|arc| arc as Arc<dyn ExceptionFilterErased>)
    })
}
