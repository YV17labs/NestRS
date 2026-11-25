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
//! reqwest error — and reqwest-middleware's, under `middleware` — where that
//! error is a link of the chain, bare, behind an `Arc` or a `Box`, or inside
//! the I/O error a reader over a body's stream makes of it, by what this crate
//! reads of it: its kind, a status by its class and its code, a decode failure
//! without its value, an I/O error by its OS's sentence or its kind, and a
//! timeout, a failed lookup, a closed connection or a refused certificate by a
//! constant sentence. A log line names what failed and why, never the URL a
//! request carried, the reason phrase a peer wrote on its status line, a value
//! its answer held or the names in its certificate; a cause this crate cannot
//! read — hyper's, a redirect policy's, a serializer's — is not said, but one
//! another crate's `SafeLink` claims, which that crate says: a body a
//! resilience policy gave up says its timeout. An error handing reqwest's
//! sentence on as its own (`#[error(transparent)]`) hides it from the chain
//! and is said as reqwest writes it: built from `reqwest::Error::without_url`
//! it holds no URL, and still a status's reason phrase and its causes' own
//! sentences, which an error naming reqwest's as its `#[source]` leaves to
//! this crate.
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
