//! HTTP credential extractors shared by bearer and basic-auth [`Strategy`](crate::Strategy) impls.

use base64::Engine as _;
use poem::{Request, http::header};

/// Pull a token out of `Authorization: Bearer <token>`, if non-empty.
pub fn bearer_token(req: &Request) -> Option<&str> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// Pull `(client_id, client_secret)` out of `Authorization: Basic <base64>`
/// (RFC 7617). The decoded `id:secret` is split on the **first** colon — a
/// secret may itself contain colons (RFC 6749 §2.3.1 client auth).
///
/// Both halves are then form-urldecoded, as RFC 6749 §2.3.1 says they were
/// encoded (OAuth 2.1 §2.4.1 keeps it).
pub fn basic_credentials(req: &Request) -> Option<(String, String)> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    // RFC 7235: auth schemes are case-insensitive.
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let encoded = encoded.trim();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    Some((form_urldecode(id), form_urldecode(secret)))
}

/// One hex digit's value, or `None` for anything RFC 3986 §2.1's `HEXDIG` does
/// not admit.
fn hex_digit(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|digit| digit as u8)
}

/// The `application/x-www-form-urlencoded` decoding of RFC 6749 Appendix B:
/// `+` is a space, `%XX` is a byte. A malformed escape is left verbatim rather
/// than rejected — this runs on a credential, and refusing to decode would turn
/// a typo into a different failure than the wrong-secret it actually is.
fn form_urldecode(raw: &str) -> String {
    if !raw.contains('%') && !raw.contains('+') {
        return raw.to_owned();
    }
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            // RFC 3986 §2.1 `HEXDIG HEXDIG`, digit by digit: `u8::from_str_radix`
            // accepts a sign, so `%+1` would decode to `\x01`.
            b'%' => match (
                bytes.get(i + 1).copied().and_then(hex_digit),
                bytes.get(i + 2).copied().and_then(hex_digit),
            ) {
                (Some(hi), Some(lo)) => {
                    out.push((hi << 4) | lo);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    // A credential that does not decode to UTF-8 is returned as it arrived, so
    // the comparison that follows sees exactly what the client sent.
    String::from_utf8(out).unwrap_or_else(|_| raw.to_owned())
}
