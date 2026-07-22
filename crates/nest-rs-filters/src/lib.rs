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
//! ```rust,ignore
//! use nest_rs_core::{Layer, injectable};
//! use nest_rs_filters::{Filter, RequestSnapshot};
//! use poem::{Response, http::StatusCode};
//! use async_trait::async_trait;
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
//!         Response::builder()
//!             .status(StatusCode::INTERNAL_SERVER_ERROR)
//!             .body(err.to_string())
//!     }
//! }
//! ```
//!
//! ## Registering globally
//!
//! ```rust,ignore
//! use nest_rs::App;
//! use nest_rs_filters::{AppBuilderFiltersExt, filter};
//!
//! App::builder()
//!     .use_filters_global([filter::<ProblemDetailsFilter>()])
//!     .module::<AppModule>()
//!     .build().await?.run().await
//! ```
#![warn(missing_docs)]

mod builder;
mod ext;
mod filter;
mod registry;

pub use builder::AppBuilderFiltersExt;
pub use ext::FilterExt;
pub use filter::{Filter, FilterEndpoint, RequestSnapshot};
pub use registry::{FilterSpec, FilterSpecs, filter};
