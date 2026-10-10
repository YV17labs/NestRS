//! OAuth 2.0 **authorization server** for nestrs — this app acting as the party
//! that *issues* tokens.
//!
//! - [`TokenError`] — §5.2's closed set of six wire codes, with the JSON envelope
//!   and the §5.1 cache directives the token endpoint owes.
//! - [`AccessTokenResponse`] — §5.1's success body, returned bare from a
//!   handler, which renders with the same directives.
//! - [`authenticate_against_registry`] — §2.3.1 client authentication against a
//!   static registry, in constant time.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod error;
mod registry;
mod token;

pub use error::TokenError;
pub use registry::{AuthenticatedClient, RegisteredClient, authenticate_against_registry};
pub use token::{AccessTokenRequest, AccessTokenResponse};
