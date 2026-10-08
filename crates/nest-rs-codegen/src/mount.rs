//! The grammar of a **declared mount path** — `#[controller(path = …)]`,
//! `#[gateway(path = …)]`, `#[mcp(path = …)]`.
//!
//! RFC 3986 §3.3 `path-absolute`, widened with `{`/`}` for a template segment
//! and narrowed twice: no percent-encoding (`%2F` mounts an address the router
//! never matches), and no interior empty segment, which the RFC permits.

use syn::LitStr;

/// The longest declared mount path.
pub(crate) const MAX_PATH_LEN: usize = 256;

/// Why a declared mount path is not one, or `None` when it is.
fn violation(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return Some("it is empty — drop the argument to take the default".to_owned());
    }
    if !raw.starts_with('/') {
        return Some(format!(
            "it does not begin with `/` — a mount path is absolute (RFC 3986 §3.3 \
             `path-absolute`), so write `/{raw}`"
        ));
    }
    if raw.len() > MAX_PATH_LEN {
        return Some(format!(
            "it is {} characters — a mount path is bounded at {MAX_PATH_LEN}, because it \
             is spliced into every route's address and into the OpenAPI document",
            raw.len(),
        ));
    }
    if raw.len() > 1 && raw.contains("//") {
        return Some(
            "it contains an empty segment (`//`) — that addresses the same resource as \
             the single-slash spelling, and only one of the two is normalized"
                .to_owned(),
        );
    }
    if let Some(bad) = raw.chars().find(|c| !is_path_char(*c)) {
        return Some(format!(
            "`{bad}` is not allowed in a mount path — RFC 3986 §3.3 spells a segment \
             from unreserved characters, sub-delimiters, `:` and `@`, plus \
             percent-encoding, **which a declared mount does not take** (a mount path \
             is written, not received: `%2F` here asks to mount an address the router \
             will never match). `{{` and `}}` are this framework's addition, for a \
             template segment."
        ));
    }
    None
}

/// One character of a declared mount path: RFC 3986 `pchar` without
/// percent-encoding, the `/` separator, or the `{`/`}` template braces.
fn is_path_char(c: char) -> bool {
    matches!(c,
        // unreserved (RFC 3986 §2.3)
        'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~'
        // sub-delims (§2.2)
        | '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '='
        // the remaining pchar members (§3.3)
        | ':' | '@'
        // the separator, and the template widening
        | '/' | '{' | '}'
    )
}

/// Refuse a declared mount path that is not one, naming the decorator, the
/// offending value and the fact from the standard that decides it.
pub fn reject_path(attr: &str, path: &LitStr) -> syn::Result<()> {
    match violation(&path.value()) {
        Some(why) => Err(syn::Error::new_spanned(
            path,
            format!(
                "{} is not a mount path: {why}",
                crate::args::site(attr, Some("path"))
            ),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_the_repo_declares_is_accepted() {
        for raw in [
            "/",
            "/users",
            "/.well-known",
            "/fund/drafts",
            "/ctx-bare",
            "/users/{id}",
            "/ws/chat",
            "/mcp",
        ] {
            assert!(
                violation(raw).is_none(),
                "rejected `{raw}`: {:?}",
                violation(raw)
            );
        }
    }

    #[test]
    fn the_four_ways_of_writing_one_wrong_are_named() {
        for (raw, expect) in [
            ("", "empty"),
            ("users", "absolute"),
            ("/a b", "not allowed"),
            ("//users", "empty segment"),
        ] {
            let why = violation(raw).unwrap_or_else(|| panic!("accepted `{raw}`"));
            assert!(why.contains(expect), "`{raw}` said: {why}");
        }
    }

    #[test]
    fn the_length_bound_is_enforced() {
        let long = format!("/{}", "a".repeat(MAX_PATH_LEN));
        assert!(violation(&long).is_some_and(|why| why.contains("bounded")));
    }
}
