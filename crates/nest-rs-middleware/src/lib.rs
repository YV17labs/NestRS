//! Named middleware vocabulary for nestrs — three categories layered over poem's
//! single `Middleware` trait:
//!
//! - [`Interceptor`] — wraps handler execution (logging, metrics, response shaping).
//! - [`Guard`] — pre-handler authorization; short-circuits with a [`Response`](poem::Response).
//! - [`Filter`] — maps inner-endpoint errors to responses.
//!
//! All three plug in via the [`EndpointExt`] extension trait. Raw
//! [`poem::Middleware`] remains available via poem's `.with()`.

mod ext;
mod filter;
mod guard;
mod interceptor;

pub use ext::EndpointExt;
pub use filter::{Filter, RequestSnapshot};
pub use guard::Guard;
pub use interceptor::{Interceptor, Next};
