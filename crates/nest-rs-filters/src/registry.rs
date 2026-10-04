//! Layer registration — typed specs the builder uses to seed the global
//! filter chain into the container.
//!
//! `FilterSpec` is a [`LayerSpec`] alias — the shared
//! shape and its `resolve` method live in `nest-rs-core`; only the typed
//! constructor and the erased trait differ per family.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::LayerSpec;

use crate::filter::Filter;

/// One entry in the `use_filters_global` list. Resolved against the live
/// container at configure time.
pub type FilterSpec = LayerSpec<dyn Filter>;

/// Construct a [`FilterSpec`] for the given filter type.
///
/// ```
/// use nest_rs_filters::{AppBuilderFiltersExt, filter};
/// # use nest_rs_core::App;
/// # use nest_rs_core::{Layer, injectable, module};
/// # use nest_rs_filters::{Filter, RequestSnapshot, async_trait};
/// # #[injectable]
/// # #[derive(Default)]
/// # struct ProblemDetailsFilter;
/// # impl Layer for ProblemDetailsFilter {}
/// # #[async_trait]
/// # impl Filter for ProblemDetailsFilter {
/// #     async fn filter(&self, _snap: &RequestSnapshot, err: poem::Error) -> poem::Response {
/// #         err.into_response()
/// #     }
/// # }
/// # #[module(providers = [ProblemDetailsFilter])]
/// # struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
///
/// App::builder()
///     .use_filters_global([filter::<ProblemDetailsFilter>()])
///     .module::<AppModule>()
/// #   .build().await?;
/// # Ok(())
/// # }
/// ```
pub fn filter<F: Filter + 'static>() -> FilterSpec {
    LayerSpec::new(TypeId::of::<F>(), std::any::type_name::<F>(), |c| {
        c.get::<F>().map(|arc| arc as Arc<dyn Filter>)
    })
}

/// The unresolved `Vec<FilterSpec>` seeded into the container by
/// `AppBuilder::use_filters_global(...)`. The HTTP shaper reads it at configure
/// time and resolves against the live container.
pub struct FilterSpecs(pub Vec<FilterSpec>);

impl nest_rs_core::layer_chain::GlobalSpecs for FilterSpecs {
    type Layer = dyn Filter;
    fn specs(&self) -> &[FilterSpec] {
        &self.0
    }
}
