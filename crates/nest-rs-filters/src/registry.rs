//! Layer registration — typed specs the builder uses to seed the global
//! filter chain into the container.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::LayerSpec;

use crate::filter::Filter;

/// One filter, resolved against the container.
pub type FilterSpec = LayerSpec<dyn Filter>;

/// Construct a [`FilterSpec`] for the given filter type.
pub fn filter<F: Filter + 'static>() -> FilterSpec {
    LayerSpec::new(TypeId::of::<F>(), std::any::type_name::<F>(), |c| {
        c.get::<F>().map(|arc| arc as Arc<dyn Filter>)
    })
}
