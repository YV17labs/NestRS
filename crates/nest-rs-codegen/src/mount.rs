//! The grammar of a **declared mount path** — `#[controller(path = …)]`,
//! `#[gateway(path = …)]`, `#[mcp(path = …)]`.
//!
//! RFC 3986 §3.3 `path-absolute`, widened with `{`/`}` for a template segment
//! and narrowed twice: no percent-encoding (`%2F` mounts an address the router
//! never matches), and no interior empty segment, which the RFC permits. What
//! is left is read with the route grammar ([`RoutePath`]).

use syn::LitStr;

use crate::route_path::RoutePath;

/// What a declared mount path holds beyond literal text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountPath {
    /// A controller's: route parameters, since every route's template opens
    /// with it, and no catch-all, since its routes follow it.
    Prefix,
    /// A gateway's or an MCP host's: literal text alone, since the surface is
    /// mounted whole at one address.
    Literal,
}

/// The longest declared mount path.
pub(crate) const MAX_PATH_LEN: usize = 256;

impl MountPath {
    /// Why `raw` is not a declared mount path of this kind — the clause after
    /// a decorator's site — or `None` when it is one.
    pub fn refusal(self, raw: &str) -> Option<String> {
        violation(raw, self)
    }
}

/// Why a declared mount path is not one, or `None` when it is.
fn violation(raw: &str, kind: MountPath) -> Option<String> {
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
    // Before the grammar, whose remedy would be a template this path refuses.
    if kind == MountPath::Literal && raw.contains(['{', '}', ':', '*']) {
        return Some(LITERAL.to_owned());
    }
    match RoutePath::read(raw) {
        Err(why) => Some(why),
        Ok(route) if route.has_catch_all() => Some(
            "it ends with a catch-all, and a controller's routes follow its path — declare \
             `{*rest}` on a route"
                .to_owned(),
        ),
        Ok(_) => None,
    }
}

/// Why a literal mount path holds no template syntax, worded as
/// `nest_rs_http::RouteTemplate` words it for a path mounted at boot.
const LITERAL: &str = "it is mounted at one literal address, so it holds no parameter, no brace, and no `:`, `*` \
     or `<`";

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
pub fn reject_path(attr: &str, path: &LitStr, kind: MountPath) -> syn::Result<()> {
    match violation(&path.value(), kind) {
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
            "/ws/chat",
            "/mcp",
        ] {
            for kind in [MountPath::Prefix, MountPath::Literal] {
                assert!(
                    violation(raw, kind).is_none(),
                    "rejected `{raw}`: {:?}",
                    violation(raw, kind)
                );
            }
        }
        assert!(violation("/orgs/{org}/members", MountPath::Prefix).is_none());
    }

    #[test]
    fn the_four_ways_of_writing_one_wrong_are_named() {
        for (raw, expect) in [
            ("", "empty"),
            ("users", "absolute"),
            ("/a b", "not allowed"),
            ("//users", "empty segment"),
        ] {
            let why =
                violation(raw, MountPath::Prefix).unwrap_or_else(|| panic!("accepted `{raw}`"));
            assert!(why.contains(expect), "`{raw}` said: {why}");
        }
    }

    #[test]
    fn the_length_bound_is_enforced() {
        let long = format!("/{}", "a".repeat(MAX_PATH_LEN));
        assert!(violation(&long, MountPath::Literal).is_some_and(|why| why.contains("bounded")));
    }

    #[test]
    fn a_mount_path_reads_the_route_grammar() {
        for (raw, kind, expect) in [
            ("/orgs/:org", MountPath::Prefix, "write `/orgs/{org}`"),
            ("/files/*rest", MountPath::Prefix, "write `/files/{*rest}`"),
            ("/tools/:tenant", MountPath::Literal, "one literal address"),
            (
                "/files/{*rest}",
                MountPath::Prefix,
                "routes follow its path",
            ),
            ("/ws/{room}", MountPath::Literal, "one literal address"),
            ("/ws/{{x}}", MountPath::Literal, "one literal address"),
        ] {
            let why = violation(raw, kind).unwrap_or_else(|| panic!("accepted `{raw}`"));
            assert!(why.contains(expect), "`{raw}` said: {why}");
        }
    }
}
