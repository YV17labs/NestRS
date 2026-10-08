//! Layer registration — typed specs the builder uses to seed the global
//! exception-filter chain into the container.

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
/// ```
/// use nest_rs_exception_filters::{AppBuilderExceptionFiltersExt, exception_filter};
/// # use nest_rs_core::App;
/// # use nest_rs_core::{Layer, injectable, module};
/// # use nest_rs_exception_filters::{ExceptionFilter, async_trait};
/// # #[derive(Debug, thiserror::Error)]
/// # #[error("domain error")]
/// # struct DomainError;
/// # #[injectable]
/// # #[derive(Default)]
/// # struct DomainErrorFilter;
/// # impl Layer for DomainErrorFilter {}
/// # #[async_trait]
/// # impl ExceptionFilter for DomainErrorFilter {
/// #     type Exception = DomainError;
/// #     async fn catch(&self, _err: DomainError) -> poem::Response {
/// #         poem::http::StatusCode::BAD_REQUEST.into()
/// #     }
/// # }
/// # #[module(providers = [DomainErrorFilter])]
/// # struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
///
/// App::builder()
///     .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
///     .module::<AppModule>()
/// #   .build().await?;
/// # Ok(())
/// # }
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

impl nest_rs_core::layer_chain::GlobalSpecs for ExceptionFilterSpecs {
    type Layer = dyn ExceptionFilterErased;
    fn specs(&self) -> &[ExceptionFilterSpec] {
        &self.0
    }
}
