//! HTTP credential extractors shared by bearer and basic-auth [`Strategy`] impls.

use base64::Engine as _;
use poem::{http::header, Request};

/// Pull the token out of an `Authorization: Bearer <token>` header, if present
/// and non-empty. The building block of any bearer/JWT [`Strategy`](super::Strategy).
pub fn bearer_token(req: &Request) -> Option<&str> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    (!token.is_empty()).then_some(token)
}

/// Pull `(client_id, client_secret)` out of an `Authorization: Basic <base64>`
/// header (RFC 7617), if present and well-formed. The building block of HTTP Basic
/// schemes — chiefly OAuth2 client authentication (RFC 6749 §2.3.1), the
/// RFC-preferred way for a confidential client to authenticate at the token
/// endpoint. The decoded `id:secret` is split on the **first** colon, so a secret
/// may itself contain colons. Symmetric to [`bearer_token`].
pub fn basic_credentials(req: &Request) -> Option<(String, String)> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?.trim();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    Some((id.to_owned(), secret.to_owned()))
}
