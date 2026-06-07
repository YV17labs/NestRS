//! # nest-rs-interceptors
//!
//! HTTP interceptors — the wrap-handler slot of the Layer System.
//!
//! An [`Interceptor`] sees the request before the handler runs and the response
//! after, in a single `intercept(req, next)` call. It is a [`Layer`] sub-trait,
//! so global + per-scope declarations dedup by [`TypeId`](std::any::TypeId) at
//! mount time (broadest scope wins).
//!
//! ## Defining an interceptor
//!
//! ```rust,ignore
//! use nest_rs_core::{Layer, injectable};
//! use nest_rs_interceptors::{Interceptor, Next};
//! use poem::{Request, Response, Result};
//! use async_trait::async_trait;
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
//!         resp.headers_mut().insert("Server-Timing",
//!             format!("total;dur={dur}").parse().unwrap());
//!         Ok(resp)
//!     }
//! }
//! ```
//!
//! ## Registering globally
//!
//! ```rust,ignore
//! use nest_rs::App;
//! use nest_rs_interceptors::{AppBuilderInterceptorsExt, interceptor};
//!
//! App::builder()
//!     .use_interceptors_global([interceptor::<ServerTiming>()])
//!     .module::<AppModule>()
//!     .build().await?.run().await
//! ```

mod builder;
mod ext;
mod interceptor;
mod registry;

pub use builder::AppBuilderInterceptorsExt;
pub use ext::InterceptorExt;
pub use interceptor::{Interceptor, InterceptorEndpoint, Next};
pub use registry::{InterceptorSpec, InterceptorSpecs, interceptor};
