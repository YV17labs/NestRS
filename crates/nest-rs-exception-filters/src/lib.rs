//! # nest-rs-exception-filters
//!
//! The poem-typed exception filter `nest_rs::core::ExceptionFilter` replaced:
//! that one runs on every edge, through the one pipeline, and
//! `#[use_exception_filters]` and `use_exception_filters_global` bind it.
//! Nothing in the framework runs this one any more; the crate leaves in 7.0.
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
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod erased;
mod exception_filter;
mod registry;

pub use erased::ExceptionFilterErased;
pub use exception_filter::ExceptionFilter;
pub use registry::{ExceptionFilterSpec, exception_filter};
// Re-exported so an `ExceptionFilter` impl needs no `async-trait` dependency of its own.
pub use async_trait::async_trait;
