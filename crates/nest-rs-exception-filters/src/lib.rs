//! # nest-rs-exception-filters
//!
//! [`ExceptionFilter`] catches a single typed exception. Unlike a `Filter`
//! from `nest-rs-filters` (which unconditionally maps every inner error to a
//! response), an `ExceptionFilter` declares the concrete error type it claims
//! via its [`ExceptionFilter::Exception`] associated type and only catches
//! matching errors. Non-matching errors keep flowing through any outer filter.
//!
//! **Pick this one by default**: a typed catch expresses the common intent
//! (map *this* domain error to *that* status) and leaves everything else to
//! flow outward. Reach for the untyped `Filter` only when the mapping is
//! genuinely unconditional.
//!
//! Dispatch is via `poem::Error::downcast::<Exception>()` — anything carryable
//! as a `Box<dyn std::error::Error + Send + Sync + 'static>` is catchable.
//!
//! There is no `.except_filter(_)` shim: a typed `Self::Exception` cannot be
//! erased through a poem endpoint wrapper, so the dispatcher in `nest-rs-guards`
//! holds the typed list and attempts each downcast in order.
//!
//! ## Defining an exception filter
//!
//! ```
//! use nest_rs_core::{Layer, injectable};
//! use nest_rs_exception_filters::{ExceptionFilter, async_trait};
//! use poem::{Response, http::StatusCode};
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
//! # fn main() {}
//! ```
//!
//! ## Registering globally
//!
//! Register with `App::builder().use_exception_filters_global([...])`
//! ([`AppBuilderExceptionFiltersExt`]); the example on
//! [`exception_filter`](fn@exception_filter) runs it.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod builder;
mod erased;
mod exception_filter;
mod registry;

pub use builder::AppBuilderExceptionFiltersExt;
pub use erased::ExceptionFilterErased;
pub use exception_filter::ExceptionFilter;
pub use registry::{ExceptionFilterSpec, ExceptionFilterSpecs, exception_filter};
// Re-exported so an `ExceptionFilter` impl needs no `async-trait` dependency of its own.
pub use async_trait::async_trait;
