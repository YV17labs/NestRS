//! W3C Trace Context and client telemetry on the reqwest client an app builds
//! itself: [`ReqwestMiddleware`], added once where the client is built, carries
//! the unit of work's trace onto every request and records each as the client
//! operation the HTTP conventions name.
//!
//! The framework opens no outbound HTTP surface of its own for an app: the app
//! builds and injects its reqwest client, in reqwest's own words. The
//! `middleware` feature, on by default, brings [`ReqwestMiddleware`]; the
//! framework's own reqwest clients take this crate without it.
//!
//! In a program linking this crate, [`nest_rs_core::error_message`] says a
//! reqwest error — and reqwest-middleware's, under `middleware` — without the
//! URL its own sentence quotes, wherever a chain holds one: a log line names
//! what failed and why, never the path and the query a request carried.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod delay_seconds;
#[cfg(feature = "middleware")]
mod middleware;
mod retry_after;
mod safe_link;
mod traced;

#[cfg(feature = "middleware")]
pub use middleware::ReqwestMiddleware;

/// Tier 2: what the framework's own reqwest clients send through. Not API.
#[doc(hidden)]
pub mod __private {
    pub use crate::delay_seconds::delay_seconds;
    pub use crate::retry_after::retry_after;
    pub use crate::traced::{Resent, Traced, traced};
}
