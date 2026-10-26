//! OAuth 2.0 **authorization server** for nestrs — this app acting as the party
//! that *issues* tokens.
//!
//! - [`ClientAuthenticationGuard`] — §2.3's client authentication at the token
//!   endpoint, over an authn [`Strategy`](nest_rs_authn::Strategy), refusing as
//!   §5.2 answers: `invalid_client`, by the [`TokenEndpointAuthMethod`] the
//!   client attempted.
//! - [`AccessTokenRequest`] — §4.4.2's request, read off its form as a handler
//!   parameter; one that does not parse is §5.2's `invalid_request`, saying
//!   where.
//! - [`TokenError`] — §5.2's closed set of six wire codes, with the JSON envelope
//!   and the §5.1 cache directives the token endpoint owes.
//! - [`AccessTokenResponse`] — §5.1's success body, stating the scope granted
//!   when it differs from the request, returned bare from a handler, which
//!   renders with the same directives.
//! - [`authenticate_against_registry`] — §2.3.1 client authentication against a
//!   static registry, in constant time, for a strategy to call.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — RFC 6749's token endpoint: the `warn` for each
/// client [`ClientAuthenticationGuard`] refuses. The cause a `server_error`
/// answer withholds is the HTTP edge's line, on `nest_rs::http`, filed once
/// the answer is final.
pub const TARGET: &str = "nest_rs::oauth::server";

mod error;
mod guard;
mod registry;
mod token;

pub use error::{TokenEndpointAuthMethod, TokenError};
pub use guard::ClientAuthenticationGuard;
pub use registry::{AuthenticatedClient, RegisteredClient, authenticate_against_registry};
pub use token::{AccessTokenRequest, AccessTokenResponse};
