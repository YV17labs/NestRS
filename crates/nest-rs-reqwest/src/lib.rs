//! W3C Trace Context and client telemetry on the reqwest client an app builds
//! itself: [`ReqwestMiddleware`], added once where the client is built, carries
//! the unit of work's trace onto every request and records each as the client
//! operation the HTTP conventions name.
//!
//! The framework opens no outbound HTTP surface of its own for an app: the app
//! builds and injects its reqwest client, in reqwest's own words. The
//! `middleware` feature, on by default, brings [`ReqwestMiddleware`]; the
//! framework's own reqwest clients take this crate without it.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

#[cfg(feature = "middleware")]
mod middleware;
mod traced;

#[cfg(feature = "middleware")]
pub use middleware::ReqwestMiddleware;

/// Tier 2: what the framework's own reqwest clients send through. Not API.
#[doc(hidden)]
pub mod __private {
    pub use crate::traced::traced;
}
