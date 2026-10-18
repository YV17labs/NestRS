//! # nest-rs-filters
//!
//! The poem-typed error mapper that `nest_rs::core::ExceptionFilter` replaced:
//! an exception filter whose `Exception` is `dyn Error + Send + Sync` catches
//! every failure, through the one pipeline, and `#[use_filters]` is refused
//! naming `#[use_exception_filters]`. Nothing in the framework runs a `Filter`
//! any more; the crate leaves in 7.0.
//!
//! ## Defining a filter
//!
//! ```
//! use nest_rs_core::{Layer, error_message, injectable, tracing};
//! use nest_rs_filters::{Filter, RequestSnapshot, async_trait};
//! use nest_rs_http::ProblemDetails;
//! use poem::{IntoResponse, Response};
//! # mod app { pub const TARGET: &str = "app::http"; }
//!
//! #[injectable]
//! #[derive(Default)]
//! pub struct ProblemDetailsFilter;
//!
//! impl Layer for ProblemDetailsFilter {}
//!
//! #[async_trait]
//! impl Filter for ProblemDetailsFilter {
//!     async fn filter(&self, _snap: &RequestSnapshot, err: poem::Error) -> Response {
//!         tracing::error!(target: app::TARGET, error = %error_message(&err), "request failed");
//!         ProblemDetails::from_status(err.status()).into_response()
//!     }
//! }
//! # fn main() {}
//! ```
//!
//! The reply carries the status alone: an error's text can quote the request
//! it failed on, so it goes to the log and never to the client.
//!
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod ext;
mod filter;
mod registry;

pub use ext::FilterExt;
pub use filter::{Filter, FilterChain, FilterEndpoint, RequestSnapshot};
pub use registry::{FilterSpec, filter};
// Re-exported so a `Filter` impl needs no `async-trait` dependency of its own.
pub use async_trait::async_trait;
