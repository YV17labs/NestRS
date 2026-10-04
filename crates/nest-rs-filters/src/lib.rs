//! # nest-rs-filters
//!
//! HTTP filters — the error-mapping slot of the Layer System.
//!
//! A [`Filter`] runs only on the error path: when the inner endpoint returns
//! `Err(poem::Error)`, the filter maps the error to a [`Response`](poem::Response).
//! Successful responses pass through unchanged. `Filter` is a [`Layer`](nest_rs_core::Layer)
//! sub-trait so global + per-scope declarations dedup by
//! [`TypeId`](std::any::TypeId) at mount time.
//!
//! # Which of the two do I want?
//!
//! **`Filter` maps *every* error; `ExceptionFilter` maps *one type* of error.**
//! Reach for `nest_rs_exception_filters::ExceptionFilter` first — a typed catch
//! is what a handler usually wants (map `PostAlreadyPublished` to a `409`) and
//! it leaves every other error alone. Reach for `Filter` when the mapping is
//! unconditional: a crate-wide envelope, a last-resort `500` shaper.
//!
//! The name is the Layer System's slot name, kept even though "filter" reads
//! like selection in ordinary English: the five families (`Guard`, `Pipe`,
//! `Interceptor`, `Filter`, `ExceptionFilter`) are one vocabulary, and it is
//! the NestJS one. What matters is that this trait **only ever runs on the
//! error path** — a successful response never reaches it.
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
//! ## Registering globally
//!
//! Register with `App::builder().use_filters_global([...])`
//! ([`AppBuilderFiltersExt`]); the example on [`filter`](fn@filter) runs it.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod builder;
mod ext;
mod filter;
mod registry;

pub use builder::AppBuilderFiltersExt;
pub use ext::FilterExt;
pub use filter::{Filter, FilterChain, FilterEndpoint, RequestSnapshot};
pub use registry::{FilterSpec, FilterSpecs, filter};
// Re-exported so a crate writing an `Filter` impl needs no direct
// `async-trait` dependency of its own. `nest-rs-http`, `nest-rs-queue` and
// `nest-rs-ws` already do this; the layer crates did not, so the one import a
// reader needed most was the one no page could name — and the miss cascades
// (without the attribute, every trait method reports a lifetime mismatch, so
// the real cause is buried under four unrelated errors).
pub use async_trait::async_trait;
