//! [`ResourceBinding`] — what the protected resource an app serves expects of
//! every access token it verifies.

/// The `aud` and `iss` RFC 9068 §4 has a resource server require: its own
/// identifier, and one of the authorization servers it trusts, each compared
/// exactly.
///
/// A module that knows the resource provides it as a factory output, and
/// [`AuthnModule`](crate::AuthnModule) builds its [`JwtService`](crate::JwtService)
/// from it: the audience replaces the configured one, and the issuers are the
/// ones a token may name.
#[derive(Clone, Debug)]
pub struct ResourceBinding {
    /// The `aud` every token must carry.
    pub audience: String,
    /// What a token's `iss` must be one of; a token naming none is refused.
    pub issuers: Vec<String>,
}
