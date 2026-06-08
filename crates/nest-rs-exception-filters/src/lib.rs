//! # nest-rs-exception-filters
//!
//! [`ExceptionFilter`] catches a single typed exception. Unlike a [`Filter`]
//! from `nest-rs-filters` (which unconditionally maps every inner error to a
//! response), an `ExceptionFilter` declares the concrete error type it claims
//! via its [`ExceptionFilter::Exception`] associated type and only catches
//! matching errors. Non-matching errors keep flowing through any outer filter.
//!
//! Dispatch is via `poem::Error::downcast::<Exception>()` — anything carryable
//! as a `Box<dyn std::error::Error + Send + Sync + 'static>` is catchable.
//!
//! Unlike [`Filter`](nest_rs_filters::Filter), there is no `ExceptionFilterExt`
//! `.except_filter(_)` shim because an exception filter is **typed** — its
//! `Self::Exception` cannot be erased through a poem endpoint wrapper without
//! losing the downcast. Wiring runs through `ScopedExceptionFilterSpec` + the
//! shared dispatcher in `nest-rs-guards`, which holds the typed list and
//! attempts each downcast in order.
//!
//! ## Defining an exception filter
//!
//! ```rust,ignore
//! use nest_rs_core::{Layer, injectable};
//! use nest_rs_exception_filters::ExceptionFilter;
//! use poem::{Response, http::StatusCode};
//! use async_trait::async_trait;
//!
//! #[derive(Debug, thiserror::Error)]
//! #[error("domain error")]
//! pub struct DomainError;
//!
//! #[injectable]
//! #[derive(Default)]
//! pub struct DomainErrorFilter;
//!
//! impl Layer for DomainErrorFilter {}
//!
//! #[async_trait]
//! impl ExceptionFilter for DomainErrorFilter {
//!     type Exception = DomainError;
//!     async fn catch(&self, _err: DomainError) -> Response {
//!         Response::builder().status(StatusCode::BAD_REQUEST).body("domain error")
//!     }
//! }
//! ```
//!
//! ## Registering globally
//!
//! ```rust,ignore
//! use nest_rs::App;
//! use nest_rs_exception_filters::{AppBuilderExceptionFiltersExt, exception_filter};
//!
//! App::builder()
//!     .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
//!     .module::<AppModule>()
//!     .build().await?.run().await
//! ```

mod builder;
mod erased;
mod exception;
mod registry;

pub use builder::AppBuilderExceptionFiltersExt;
pub use erased::ExceptionFilterErased;
pub use exception::ExceptionFilter;
pub use registry::{ExceptionFilterSpec, ExceptionFilterSpecs, exception_filter};
