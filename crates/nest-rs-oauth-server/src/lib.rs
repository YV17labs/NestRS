//! OAuth 2.0 **authorization server** for nestrs — this app acting as the party
//! that *issues* tokens.
//!
//! RFC 6749 §1.1 names four roles. `nest-rs-authn` answers a question that
//! precedes all four (*who is calling?*), `nest-rs-oauth-client` is the client,
//! `nest-rs-oauth-resource` is the discoverable half of the resource server,
//! and this crate is the fourth: the authorization server's own vocabulary.
//!
//! Two things live here, and they are the same conversation seen from one end:
//!
//! - [`TokenError`] — §5.2's closed set of six wire codes, plus the JSON
//!   envelope and the §5.1 cache directives the token endpoint owes. Held by the
//!   framework so an app serving `/token` spells them once rather than per
//!   deployment, and so a code outside the set — which no conforming client can
//!   branch on — is unspellable.
//! - [`authenticate_against_registry`] — §2.3.1 client authentication against a
//!   static registry, in constant time. This is the **mirror** of
//!   `nest-rs-oauth-client`: that crate presents credentials to somebody else's
//!   token endpoint, this one checks them at ours.
//!
//! **Why not `nest-rs-authn`.** That crate resolves a caller's identity, and an
//! authenticated machine client has none — [`AuthenticatedClient::actor_id`]
//! returns `None`, which is the type answering authn's central question with
//! *nobody*. Keeping the two together also meant every resource server compiled
//! a token endpoint's error renderer and a credential registry it never serves.
//! `TokenError::UnsupportedGrant` and `TokenError::InvalidScope` are the tell:
//! neither is a credential verdict at all.
//!
//! Integration tests: `tests/integration/main.rs`, with paths mirroring `src/`.

#![warn(missing_docs)]

// No span target: this crate emits no events. A rejected client credential is a
// denial, and the guard that raised it already files one `warn` on
// `nest_rs::authn` — a second here would be the same event said twice. The
// constant is added by whichever of the two things above first has something of
// its own to say.

mod error;
mod registry;
mod token;

pub use error::TokenError;
pub use registry::{AuthenticatedClient, RegisteredClient, authenticate_against_registry};
pub use token::{AccessTokenRequest, AccessTokenResponse};
