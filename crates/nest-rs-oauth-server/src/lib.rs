//! OAuth 2.0 **authorization server** for nestrs — this app acting as the party
//! that *issues* tokens.
//!
//! - [`ClientAuthenticationGuard`] — §2.3's client authentication at the token
//!   endpoint, over an authn [`Strategy`](nest_rs_authn::Strategy), refusing as
//!   §5.2 answers: `invalid_client`, by the [`TokenEndpointAuthMethod`] the
//!   client attempted.
//! - [`OAuthServerModule`] and [`OAuthServerConfig`] — the clients this server
//!   registers (§2), under `<PREFIX>_OAUTH__SERVER__*`, which
//!   [`ClientSecretBasicStrategy`] authenticates against, in constant time.
//! - [`AccessTokenRequest`] — §4.4.2's request, RFC 8707's `resource` with it,
//!   read off its form as a handler parameter; one that does not parse is
//!   §5.2's `invalid_request`, saying where, and one naming a grant the
//!   endpoint does not serve `unsupported_grant_type` ([`GrantType`]).
//! - [`AuthenticatedClient`] — the client the guard attaches, which reads what
//!   its request is granted: the scope (§3.3,
//!   [`scope_for`](AuthenticatedClient::scope_for)) and the token's audience
//!   (RFC 8707, [`audience_for`](AuthenticatedClient::audience_for)).
//! - [`TokenError`] — §5.2's closed set of six wire codes and RFC 8707 §2's
//!   `invalid_target`, with the JSON envelope and the §5.1 cache directives the
//!   token endpoint owes.
//! - [`AccessTokenResponse`] — §5.1's success body, stating the scope granted
//!   when it differs from the request, returned bare from a handler, which
//!   renders with the same directives.
//!
//! The product keeps what it decides: the payload a client carries, and the
//! claims its token is minted with.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — RFC 6749's token endpoint: the `warn` for each
/// client [`ClientAuthenticationGuard`] refuses, and for each scope or
/// resource an authenticated client is refused. The cause a `server_error`
/// answer withholds is the HTTP edge's line, on `nest_rs::http`, filed once
/// the answer is final.
pub const TARGET: &str = "nest_rs::oauth::server";

mod config;
mod error;
mod guard;
mod module;
mod registry;
mod strategy;
mod token;

pub use config::OAuthServerConfig;
pub use error::{TokenEndpointAuthMethod, TokenError};
pub use guard::ClientAuthenticationGuard;
pub use module::{OAuthServerModule, OAuthServerSetup};
pub use registry::{AuthenticatedClient, RegisteredClient, authenticate_against_registry};
pub use strategy::ClientSecretBasicStrategy;
pub use token::{AccessTokenRequest, AccessTokenResponse, GrantType};
