//! The grammar of a **declared mount path** — `#[controller(path = …)]`,
//! `#[gateway(path = …)]`, `#[mcp(path = …)]`.
//!
//! One key, three host decorators, and until this module three answers: two
//! took the literal through `require_str_lit` and checked nothing at all, while
//! the third refused only the empty string. So `#[controller(path = "")]` and
//! `#[controller(path = "a b")]` compiled, mounted, and were logged and
//! documented under an address no client can name.
//!
//! **The sibling key in the same attribute list is what decides this.**
//! `version` — declared beside `path` on `#[controller]` and `#[gateway]` — has
//! a shared parser, a shared grammar, a shared bound and a wire-side pin test,
//! and [`versioning`](crate::versioning) gives the reason: *"A version is
//! spliced into a URL path and echoed into an `operationId`, so it is bounded
//! on both sides of the wire… A declaration the framework accepts and its own
//! wire half rejects is the worst place for the two to disagree."* A declared
//! path is spliced into the same URL, at the same two decorators, and was
//! bounded by nothing.
//!
//! **The grammar is RFC 3986 §3.3's, narrowed to what a mount can be.** A path
//! is `path-absolute`: it begins with `/`, and every character after it is a
//! `pchar` (unreserved / sub-delims / `:` / `@`) or a `/` separator — plus `{`
//! and `}`, which are not `pchar` at all and are this framework's documented
//! widening for a template segment (`/users/{id}`). Percent-encoding is
//! deliberately **not** accepted: a mount path is written by the developer, not
//! received from the wire, and `%2F` in a declaration is a request to mount an
//! address the router will never match.
//!
//! **A leading `//` is the standard's own refusal; an interior one is ours.**
//! §3.3 spells an absolute path `"/" [ segment-nz *( "/" segment ) ]`, so the
//! first segment may not be empty — `//users` would parse as an authority, not
//! a path. An *interior* or trailing empty segment the RFC permits, and this
//! grammar still refuses it: two spellings of one address, of which the mount
//! normalizes one, is an ambiguity a declaration has no reason to carry. That
//! second half is the framework's narrowing and is stated as such rather than
//! attributed to the standard.

use syn::LitStr;

/// The longest declared mount path.
///
/// `pub(crate)` and not `pub`, unlike
/// [`MAX_VERSION_LEN`](crate::versioning::MAX_VERSION_LEN) which it otherwise
/// mirrors: that one is `pub` because `nest-rs-http` carries the wire half of
/// the same grammar and a test pins the two together. A mount path has no wire
/// half — the router matches what was mounted — so exporting this would be an
/// API promising a symmetry that does not exist here.
pub(crate) const MAX_PATH_LEN: usize = 256;

/// Why a declared mount path is not one, or `None` when it is.
///
/// Returns the *fact*, not a sentence: the caller owns the decorator's name and
/// [`reject_path`] is what assembles the two.
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

/// One character of a declared mount path: RFC 3986 `pchar`, the `/` separator,
/// or this framework's `{`/`}` template braces.
///
/// Percent-encoding is excluded deliberately — see the module doc.
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
///
/// Spanned at the literal, so the caret lands on the value rather than on the
/// item — the span defect six of this repo's refusals shipped before their
/// sentences were shared.
pub fn reject_path(attr: &str, path: &LitStr) -> syn::Result<()> {
    match violation(&path.value()) {
        Some(why) => Err(syn::Error::new_spanned(
            path,
            format!("#[{attr}] `path` is not a mount path: {why}"),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every mount path either workspace declares today, so the grammar is
    /// derived from what the framework actually mounts rather than from a guess
    /// about what it might.
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

    /// A bound nothing reads is a bound that drifts.
    #[test]
    fn the_length_bound_is_enforced() {
        let long = format!("/{}", "a".repeat(MAX_PATH_LEN));
        assert!(violation(&long).is_some_and(|why| why.contains("bounded")));
    }
}
