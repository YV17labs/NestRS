//! # nest-rs-interceptors
//!
//! Interceptors — the wrap-handler slot of the Layer System.
//!
//! An [`Interceptor`] sees the inputs before the handler runs and the
//! outputs after. `intercept` (HTTP) is the only entry the framework wires
//! today: a **global** interceptor wraps the whole routing tree at the
//! transport edge (so it also observes 404s, guard denials, and
//! self-mounted surfaces — a GraphQL `POST` or WS upgrade is an HTTP
//! request); a **controller / method** interceptor wraps its handler,
//! inside the guard chain. There is no per-resolver / per-message seam —
//! a former reserved one was removed until it is actually wired.
//!
//! `Interceptor` is a [`Layer`](nest_rs_core::Layer) sub-trait, so global + per-scope
//! declarations dedup by [`TypeId`](std::any::TypeId) at mount time
//! (broadest scope wins — one execution, at the broadest scope's site).
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
//!         resp.headers_mut().insert(
//!             "Server-Timing",
//!             format!("total;dur={dur}")
//!                 .parse()
//!                 .map_err(poem::error::InternalServerError)?,
//!         );
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
#![warn(missing_docs)]

mod builder;
mod ext;
mod interceptor;
mod registry;

pub use builder::AppBuilderInterceptorsExt;
pub use ext::InterceptorExt;
pub use interceptor::{Interceptor, InterceptorChain, InterceptorEndpoint, Next};
pub use registry::{InterceptorSpec, InterceptorSpecs, interceptor};
// Re-exported so a crate writing an `Interceptor` impl needs no direct
// `async-trait` dependency of its own. `nest-rs-http`, `nest-rs-queue` and
// `nest-rs-ws` already do this; the layer crates did not, so the one import a
// reader needed most was the one no page could name — and the miss cascades
// (without the attribute, every trait method reports a lifetime mismatch, so
// the real cause is buried under four unrelated errors).
pub use async_trait::async_trait;
