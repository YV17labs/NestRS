//! HTTP credential extractors shared by bearer and basic-auth [`Strategy`] impls.

use base64::Engine as _;
use poem::{Request, http::header};

/// Pull a token out of `Authorization: Bearer <token>`, if non-empty.
pub fn bearer_token(req: &Request) -> Option<&str> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    (!token.is_empty()).then_some(token)
}

/// Pull `(client_id, client_secret)` out of `Authorization: Basic <base64>`
/// (RFC 7617). The decoded `id:secret` is split on the **first** colon — a
/// secret may itself contain colons (RFC 6749 §2.3.1 client auth).
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
