//! [`RoutePath`] — a `#[routes]` path read with nestrs's route grammar, OpenAPI
//! path templating, so the address the macro checks is the one served, logged
//! and documented. `nest_rs_http::RouteTemplate` is its runtime twin, for a
//! template that exists only at boot; one corpus pins the two
//! (`nest-rs-http`'s `route_template` tests).
//!
//! - runs of `/` collapse, and a missing leading `/` is added;
//! - a trailing `/` is not part of the address, except on the root, since the
//!   edge trims a request's before routing;
//! - a segment is literal text, then at most one parameter: `{name}`, which
//!   may follow literal text (`/@{handle}`) and ends its segment;
//! - `{*name}` is the rest of the path: named, last, and never empty;
//! - `{{` and `}}` are literal braces.
//!
//! Refused, each with the 7.0 spelling where there is one: 6.x's `:name` and
//! `*rest`, a parameter carrying a pattern (`:id<\d+>`, `{id:\d+}`), text after
//! a parameter in its segment, and a catch-all before the end. A literal `:`,
//! `*` or `<` stays refused while poem routes, which would read it as a
//! parameter (`.claude/decisions/route-template-grammar.md`).
//!
//! The **identity** drops parameter names, which decide nothing about routing.

/// One route path, read with the route grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePath {
    identity: String,
    template: String,
    catch_all: bool,
}

/// The parameter a segment ends with.
enum Parameter<'a> {
    Named(&'a str),
    Rest(&'a str),
}

const PATTERN: &str =
    "a route parameter carries no pattern — parse it with its type (`Path<u64>`) or a pipe";
const UNCLOSED: &str = "a `{` opens a parameter that no `}` closes — a literal brace is `{{`";
const UNOPENED: &str = "a `}` closes no parameter — a literal brace is `}}`";
const UNNAMED_6X: &str = "a `:` is the 6.x parameter syntax, and names none — a parameter is \
                          `{name}`";

impl RoutePath {
    /// Read `written` with the route grammar, or the sentence refusing it —
    /// `` `/users/:id`: `:id` is the 6.x parameter syntax — write `/users/{id}` `` —
    /// which a decorator prints after its site.
    pub fn parse(written: &str) -> Result<Self, String> {
        Self::read(written).map_err(|why| format!("`{written}`: {why}"))
    }

    /// [`parse`](Self::parse), refusing with the reason alone.
    pub(crate) fn read(written: &str) -> Result<Self, String> {
        let normalized = normalize(written);
        // The edge trims a request's trailing slash before routing, so a route
        // declared with one names the address without it.
        let address = match normalized.strip_suffix('/') {
            Some(rest) if !rest.is_empty() => rest,
            _ => normalized.as_str(),
        };
        let mut route = Self {
            identity: String::with_capacity(address.len()),
            template: String::with_capacity(address.len()),
            catch_all: false,
        };
        let mut segments = address.split('/').skip(1).peekable();
        while let Some(text) = segments.next() {
            let last = segments.peek().is_none();
            let (literal, parameter) = segment(text, last, written)?;
            for out in [&mut route.identity, &mut route.template] {
                out.push('/');
                out.push_str(literal);
            }
            match parameter {
                Some(Parameter::Named(name)) => {
                    route.identity.push_str("{}");
                    route.template.push('{');
                    route.template.push_str(name);
                    route.template.push('}');
                }
                Some(Parameter::Rest(name)) => {
                    route.identity.push_str("{*}");
                    route.template.push_str("{*");
                    route.template.push_str(name);
                    route.template.push('}');
                    route.catch_all = true;
                }
                None => {}
            }
        }
        Ok(route)
    }

    /// What makes two routes one address: the template with every parameter's
    /// name dropped — `/users/{id}/` and `users/{other}` are both `/users/{}`.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The template, normalized, with its parameters' names — the address
    /// served, logged and documented.
    pub fn template(&self) -> &str {
        &self.template
    }

    /// Whether the template ends with a `{*name}` catch-all.
    pub(crate) fn has_catch_all(&self) -> bool {
        self.catch_all
    }
}

/// Collapse runs of `/` and add a missing leading one, before anything is read.
fn normalize(written: &str) -> String {
    let mut out = String::with_capacity(written.len() + 1);
    if !written.starts_with('/') {
        out.push('/');
    }
    for c in written.chars() {
        if c == '/' && out.ends_with('/') {
            continue;
        }
        out.push(c);
    }
    out
}

/// One segment: its literal text as the template spells it (`{{` kept), then
/// the parameter it ends with.
fn segment<'a>(
    text: &'a str,
    last: bool,
    written: &str,
) -> Result<(&'a str, Option<Parameter<'a>>), String> {
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        let next = text[at + c.len_utf8()..].chars().next();
        match c {
            '{' | '}' if next == Some(c) => at += 2,
            '}' => return Err(UNOPENED.to_owned()),
            '{' => return parameter(&text[at + 1..], last).map(|p| (&text[..at], Some(p))),
            ':' => {
                let rest = &text[at + 1..];
                let name = &rest[..rest.find(['<', '*']).unwrap_or(rest.len())];
                return Err(if rest[name.len()..].starts_with('<') {
                    PATTERN.to_owned()
                } else if name.is_empty() {
                    UNNAMED_6X.to_owned()
                } else {
                    format!(
                        "`:{name}` is the 6.x parameter syntax — write `{}`",
                        rewrite_6x(written)
                    )
                });
            }
            '*' => {
                return Err(format!(
                    "`{}` is the 6.x catch-all syntax — write `{}`",
                    &text[at..],
                    rewrite_6x(written)
                ));
            }
            '<' => return Err(PATTERN.to_owned()),
            c => at += c.len_utf8(),
        }
    }
    Ok((text, None))
}

/// The parameter whose `{` came just before `body`, which runs to the end of
/// its segment.
fn parameter(body: &str, last: bool) -> Result<Parameter<'_>, String> {
    let close = match body.find(['{', '}']) {
        Some(close) if body[close..].starts_with('}') => close,
        _ => return Err(UNCLOSED.to_owned()),
    };
    let (inside, after) = (&body[..close], &body[close + 1..]);
    let (rest, name) = match inside.strip_prefix('*') {
        Some(name) => (true, name),
        None => (false, inside),
    };
    if name.contains([':', '<', '>', '*']) {
        return Err(PATTERN.to_owned());
    }
    if name.is_empty() {
        return Err(match rest {
            true => "`{*}` names no catch-all — write `{*rest}`",
            false => "`{}` names no parameter — write `{name}`",
        }
        .to_owned());
    }
    if rest && (!after.is_empty() || !last) {
        return Err(format!(
            "`{{{inside}}}` comes last — a catch-all takes the rest of the path"
        ));
    }
    if !after.is_empty() {
        return Err(format!(
            "`{{{inside}}}` ends its segment — a parameter reads up to the next `/`"
        ));
    }
    Ok(match rest {
        true => Parameter::Rest(name),
        false => Parameter::Named(name),
    })
}

/// `written` with each 6.x `:name` as `{name}` and each `*rest` as `{*rest}`.
fn rewrite_6x(written: &str) -> String {
    let mut out = String::with_capacity(written.len() + 8);
    let mut rest = written;
    while let Some(at) = rest.find([':', '*']) {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let name = &after[..after
            .find(['/', '<', '*', ':', '{', '}'])
            .unwrap_or(after.len())];
        match (rest[at..].starts_with('*'), name.is_empty()) {
            (true, true) => out.push_str("{*rest}"),
            (true, false) => {
                out.push_str("{*");
                out.push_str(name);
                out.push('}');
            }
            (false, _) => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[name.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &str) -> (String, String) {
        let route = RoutePath::parse(path).unwrap_or_else(|why| panic!("{why}"));
        (route.identity().to_owned(), route.template().to_owned())
    }

    fn refusal(path: &str) -> String {
        RoutePath::parse(path).expect_err("refused")
    }

    #[test]
    fn spellings_of_one_address_share_an_identity() {
        for (a, b) in [
            ("/t", "t"),
            ("/u", "/u/"),
            ("/a//b", "/a/b"),
            ("/q/{id}", "/q/{other}"),
            ("/q/{id}/", "q/{other}"),
            ("/@{handle}", "/@{name}"),
            ("/f/{*rest}", "/f/{*tail}"),
            ("/", ""),
            ("/", "//"),
        ] {
            assert_eq!(read(a).0, read(b).0, "{a} / {b}");
        }
    }

    #[test]
    fn addresses_the_router_keeps_apart_do_not_share_one() {
        for (a, b) in [
            ("/Users", "/users"),
            ("/q/{id}", "/q/{id}/x"),
            ("/f/{*rest}", "/f/{id}"),
            ("/u/{id}", "/u/@{id}"),
            ("/b/{{id}}", "/b/{id}"),
        ] {
            assert_ne!(read(a).0, read(b).0, "{a} / {b}");
        }
    }

    #[test]
    fn the_template_keeps_the_names_and_drops_the_slip() {
        assert_eq!(read("q/{id}/").1, "/q/{id}");
        assert_eq!(read("/@{handle}/x").1, "/@{handle}/x");
        assert_eq!(read("/f/{*rest}").1, "/f/{*rest}");
        assert_eq!(read("/b/{{x}}").1, "/b/{{x}}");
        assert_eq!(read("//").1, "/");
    }

    #[test]
    fn the_6x_spellings_are_refused_with_their_7_0_spelling() {
        assert_eq!(
            refusal("/users/:id"),
            "`/users/:id`: `:id` is the 6.x parameter syntax — write `/users/{id}`",
        );
        assert_eq!(
            refusal("/files/*rest"),
            "`/files/*rest`: `*rest` is the 6.x catch-all syntax — write `/files/{*rest}`",
        );
        assert_eq!(
            refusal("/@:handle/*"),
            "`/@:handle/*`: `:handle` is the 6.x parameter syntax — write `/@{handle}/{*rest}`",
        );
        assert!(refusal("/items/:").contains("names none"));
    }

    #[test]
    fn what_the_next_router_cannot_route_is_refused() {
        for (path, says) in [
            ("/n/:id<\\d+>", "carries no pattern"),
            ("/n/<\\d+>", "carries no pattern"),
            ("/n/{id:\\d+}", "carries no pattern"),
            ("/users/{id}.json", "`{id}` ends its segment"),
            ("/a/{x}{y}", "`{x}` ends its segment"),
            ("/files/{*rest}/meta", "`{*rest}` comes last"),
            ("/files/{*rest}.gz", "`{*rest}` comes last"),
            ("/a/{}", "names no parameter"),
            ("/a/{*}", "names no catch-all"),
            ("/a/{id", "no `}` closes"),
            ("/a/{i{d}", "no `}` closes"),
            ("/a/b}", "closes no parameter"),
        ] {
            let why = refusal(path);
            assert!(why.contains(says), "{path} said: {why}");
        }
    }
}
