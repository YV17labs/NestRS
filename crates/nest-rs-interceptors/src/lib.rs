//! # nest-rs-interceptors
//!
//! The poem-typed interceptor `nest_rs::core::Interceptor` replaced: that one
//! runs on every edge, through the one pipeline, and `#[use_interceptors]`
//! and `use_interceptors_global` bind it. Nothing in the framework runs this
//! one any more; the crate leaves in 7.0.
//!
//! ## Defining an interceptor
//!
//! ```
//! use nest_rs_core::{Layer, injectable};
//! use nest_rs_interceptors::{Interceptor, Next, async_trait};
//! use poem::{Request, Response, Result};
//!
//! #[injectable]
//! #[derive(Default)]
//! pub struct ServerTiming;
//!
//! impl Layer for ServerTiming {}
//!
//! #[async_trait]
//! impl Interceptor for ServerTiming {
//!     async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
//!         let started = std::time::Instant::now();
//!         let mut resp = next.run(req).await?;
//!         let dur = started.elapsed().as_millis();
//!         resp.headers_mut().insert(
//!             "Server-Timing",
//!             format!("total;dur={dur}")
//!                 .parse()
//!                 .map_err(poem::error::InternalServerError)?,
//!         );
//!         Ok(resp)
//!     }
//! }
//! # fn main() {}
//! ```
//!
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod ext;
mod interceptor;
mod registry;

pub use ext::InterceptorExt;
pub use interceptor::{Interceptor, InterceptorChain, InterceptorEndpoint, Next};
pub use registry::{InterceptorSpec, interceptor};
// Re-exported so an `Interceptor` impl needs no `async-trait` dependency of its own.
pub use async_trait::async_trait;
