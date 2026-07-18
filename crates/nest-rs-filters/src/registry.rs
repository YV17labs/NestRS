//! Layer registration — typed specs the builder uses to seed the global
//! filter chain into the container.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::Container;

use crate::filter::Filter;

/// One entry in the `use_filters_global` list. Resolved against the live
/// container at configure time.
pub struct FilterSpec {
    /// `TypeId` of the filter type — the dedup key across scopes.
    pub type_id: TypeId,
    /// The filter type's name, for boot logs and fail-secure diagnostics.
    pub name: &'static str,
    pub(crate) resolve: fn(&Container) -> Option<Arc<dyn Filter>>,
}

/// Construct a [`FilterSpec`] for the given filter type.
///
/// ```rust,ignore
/// App::builder()
///     .use_filters_global([filter::<ProblemDetailsFilter>()])
///     .module::<AppModule>()
/// ```
pub fn filter<F: Filter + 'static>() -> FilterSpec {
    FilterSpec {
        type_id: TypeId::of::<F>(),
        name: std::any::type_name::<F>(),
        resolve: |c| c.get::<F>().map(|arc| arc as Arc<dyn Filter>),
    }
}

impl FilterSpec {
    /// Resolve the filter instance from the live container, or `None` if its
    /// provider was never registered (a fail-secure boot check flags this).
    pub fn resolve(&self, container: &Container) -> Option<Arc<dyn Filter>> {
        (self.resolve)(container)
    }
}

/// The unresolved `Vec<FilterSpec>` seeded into the container by
/// `AppBuilder::use_filters_global(...)`. The HTTP shaper reads it at configure
/// time and resolves against the live container.
pub struct FilterSpecs(pub Vec<FilterSpec>);

impl FilterSpecs {
    /// The `(TypeId, name)` of every spec, for deduping the global pool against
    /// narrower-scope declarations.
    pub fn type_ids(&self) -> Vec<(TypeId, &'static str)> {
        self.0.iter().map(|s| (s.type_id, s.name)).collect()
    }
}
