//! # nest-rs-exception-filters
//!
//! [`ExceptionFilter`] — the NestJS `@Catch(...)` analog. Unlike a [`Filter`]
//! from `nest-rs-filters` (which unconditionally maps every inner error to a
//! response), an `ExceptionFilter` declares the concrete error type it claims
//! via its [`ExceptionFilter::Exception`] associated type and only catches
//! matching errors. Non-matching errors keep flowing through any outer filter.
//!
//! This crate ships the trait and the Layer System wiring. Concrete catch
//! adapters (downcasting `poem::Error` into a domain error, panic capture)
//! are app-side.
//!
//! ## Defining an exception filter
//!
//! ```rust,ignore
//! use nest_rs_core::{Layer, injectable};
//! use nest_rs_exception_filters::ExceptionFilter;
//! use poem::{Response, http::StatusCode};
//! use async_trait::async_trait;
//!
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

mod exception;

pub use exception::ExceptionFilter;
