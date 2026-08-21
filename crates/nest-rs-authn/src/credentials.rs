//! HTTP credential extractors shared by bearer and basic-auth [`Strategy`] impls.

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
/// **Both halves are then form-urldecoded**, because RFC 6749 §2.3.1 says they
/// were encoded: *"The client identifier is encoded using the
/// `application/x-www-form-urlencoded` encoding algorithm per Appendix B, and
/// the encoded value is used as the username; the client password is encoded
/// using the same algorithm and used as the password."* OAuth 2.1 §2.4.1
/// retains it verbatim. Skipping the decode made authentication succeed or fail
/// according to whether the *client library* encoded — a conforming client with
/// the secret `a b+c%d` sends `a+b%2Bc%25d`, whose constant-time comparison
/// against the stored secret then fails, and the deployment sees an
/// unexplained `invalid_client` it cannot tell from a wrong password.
pub fn basic_credentials(req: &Request) -> Option<(String, String)> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    // Scheme match mirrors `bearer_token`: RFC 7235 auth schemes are
    // case-insensitive, so `basic <b64>` is as valid as `Basic <b64>`.
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
            // RFC 3986 §2.1: `pct-encoded = "%" HEXDIG HEXDIG`. Digit-by-digit
            // rather than `u8::from_str_radix`, which accepts a leading sign —
            // `from_str_radix("+1", 16)` is `Ok(1)`, so `%+1` used to decode to
            // `\x01` and alias a second wire spelling onto one secret, while a
            // client whose literal secret contained `%+1` had it silently
            // rewritten.
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
