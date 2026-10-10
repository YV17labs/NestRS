//! [`RouteTemplate`] — a route template read with nestrs's route grammar,
//! OpenAPI path templating, at boot: what `also_mounts`, a path read from
//! config and [`HttpTransport::mount`](crate::HttpTransport::mount) declare.
//!
//! `#[routes]` reads its literals with `nest_rs_codegen::RoutePath`, which this
//! crate cannot depend on (it pulls `syn`), so the grammar is written twice and
//! pinned once, by `codegen_and_runtime_parse_one_corpus_alike` below.
//!
//! While poem routes, a template is handed to it in poem's spelling
//! ([`poem_pattern`]), and the pattern poem's router matched is read back in
//! the declared one ([`declared`]) for the span and the throttler.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use crate::error::{RouteTemplateError, TemplateRefusal};

/// A route template in nestrs's grammar — OpenAPI path templating: literal
/// segments, a `{name}` parameter ending its segment (`/users/{id}`,
/// `/@{handle}`), a final `{*name}` taking the rest of the path, and `{{` and
/// `}}` for literal braces.
///
/// Runs of `/` collapse, a missing leading `/` is added and a trailing one is
/// dropped, since the edge trims a request's before routing.
///
/// ```
/// use nest_rs_http::RouteTemplate;
///
/// let template = RouteTemplate::parse("orgs/{org}/files/{*path}/")?;
/// assert_eq!(template.as_str(), "/orgs/{org}/files/{*path}");
/// assert_eq!(template.parameters().collect::<Vec<_>>(), ["org"]);
/// assert_eq!(template.catch_all(), Some("path"));
///
/// let refused = RouteTemplate::parse("/users/:id").unwrap_err();
/// assert_eq!(
///     refused.to_string(),
///     "`/users/:id`: `:id` is the 6.x parameter syntax — write `/users/{id}`",
/// );
/// # Ok::<(), nest_rs_http::RouteTemplateError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteTemplate {
    template: String,
    identity: String,
    /// Where each `{name}`'s name sits in `template`.
    parameters: Vec<Range<usize>>,
    /// Where the `{*name}`'s name sits in `template`.
    catch_all: Option<Range<usize>>,
}

/// The parameter a segment ends with.
enum Parameter<'a> {
    Named(&'a str),
    Rest(&'a str),
}

impl RouteTemplate {
    /// Read `written` with the route grammar, or say why it is refused, in the
    /// words `#[routes]` uses for a literal.
    pub fn parse(written: &str) -> Result<Self, RouteTemplateError> {
        Self::read(written).map_err(|refusal| RouteTemplateError::new(written, refusal))
    }

    /// [`parse`](Self::parse), refusing a template that is not one literal
    /// address: what a surface mounted whole at its path declares.
    pub(crate) fn literal(written: &str) -> Result<Self, RouteTemplateError> {
        // Before the grammar, whose remedy would be a template this path refuses.
        match written.contains(['{', '}', ':', '*', '<']) {
            true => Err(RouteTemplateError::new(
                written,
                TemplateRefusal::NotLiteral,
            )),
            false => Self::parse(written),
        }
    }

    fn read(written: &str) -> Result<Self, TemplateRefusal> {
        let normalized = normalize(written);
        let address = match normalized.strip_suffix('/') {
            Some(rest) if !rest.is_empty() => rest,
            _ => normalized.as_str(),
        };
        let segments: Vec<&str> = address.split('/').skip(1).collect();
        let mut route = Self {
            template: String::with_capacity(address.len()),
            identity: String::with_capacity(address.len()),
            parameters: Vec::new(),
            catch_all: None,
        };
        for (index, text) in segments.iter().enumerate() {
            let last = index + 1 == segments.len();
            let (literal, parameter) = segment(text, last, written)?;
            for out in [&mut route.identity, &mut route.template] {
                out.push('/');
                out.push_str(&literal);
            }
            match parameter {
                Some(Parameter::Named(name)) => {
                    route.identity.push_str("{}");
                    route.template.push('{');
                    route.parameters.push(push_name(&mut route.template, name));
                    route.template.push('}');
                }
                Some(Parameter::Rest(name)) => {
                    route.identity.push_str("{*}");
                    route.template.push_str("{*");
                    route.catch_all = Some(push_name(&mut route.template, name));
                    route.template.push('}');
                }
                None => {}
            }
        }
        Ok(route)
    }

    /// The template, normalized: the address served, logged and documented.
    pub fn as_str(&self) -> &str {
        &self.template
    }

    /// The names of the `{name}` parameters, in order; the catch-all's is
    /// [`catch_all`](Self::catch_all).
    pub fn parameters(&self) -> impl Iterator<Item = &str> {
        self.parameters
            .iter()
            .map(|range| &self.template[range.clone()])
    }

    /// The name of the final `{*name}`, when the template ends with one.
    pub fn catch_all(&self) -> Option<&str> {
        self.catch_all
            .as_ref()
            .map(|range| &self.template[range.clone()])
    }

    /// What makes two templates one address: the template with every
    /// parameter's name dropped — `/users/{id}` and `/users/{key}` are both
    /// `/users/{}`.
    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }
}

impl fmt::Display for RouteTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.template)
    }
}

/// Push `name` onto `out`, answering where it landed.
fn push_name(out: &mut String, name: &str) -> Range<usize> {
    let start = out.len();
    out.push_str(name);
    start..out.len()
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
) -> Result<(String, Option<Parameter<'a>>), TemplateRefusal> {
    let mut literal = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        let next = text[at + c.len_utf8()..].chars().next();
        match c {
            '{' | '}' if next == Some(c) => {
                literal.push(c);
                literal.push(c);
                at += 2;
            }
            '}' => return Err(TemplateRefusal::Unopened),
            '{' => return parameter(&text[at + 1..], last).map(|p| (literal, Some(p))),
            ':' => {
                let rest = &text[at + 1..];
                let name = &rest[..rest.find(['<', '*']).unwrap_or(rest.len())];
                return Err(match () {
                    () if rest[name.len()..].starts_with('<') => TemplateRefusal::Pattern,
                    () if name.is_empty() => TemplateRefusal::UnnamedParameter6x,
                    () => TemplateRefusal::Parameter6x {
                        name: name.to_owned(),
                        rewrite: rewrite_6x(written),
                    },
                });
            }
            '*' => {
                return Err(TemplateRefusal::CatchAll6x {
                    written: text[at..].to_owned(),
                    rewrite: rewrite_6x(written),
                });
            }
            '<' => return Err(TemplateRefusal::Pattern),
            c => {
                literal.push(c);
                at += c.len_utf8();
            }
        }
    }
    Ok((literal, None))
}

/// The parameter whose `{` came just before `body`, which runs to the end of
/// its segment.
fn parameter(body: &str, last: bool) -> Result<Parameter<'_>, TemplateRefusal> {
    let close = match body.find(['{', '}']) {
        Some(close) if body[close..].starts_with('}') => close,
        _ => return Err(TemplateRefusal::Unclosed),
    };
    let (inside, after) = (&body[..close], &body[close + 1..]);
    let (rest, name) = match inside.strip_prefix('*') {
        Some(name) => (true, name),
        None => (false, inside),
    };
    if name.contains([':', '<', '>', '*']) {
        return Err(TemplateRefusal::Pattern);
    }
    if name.is_empty() {
        return Err(match rest {
            true => TemplateRefusal::UnnamedCatchAll,
            false => TemplateRefusal::Unnamed,
        });
    }
    if rest && (!after.is_empty() || !last) {
        return Err(TemplateRefusal::ComesLast {
            inside: inside.to_owned(),
        });
    }
    if !after.is_empty() {
        return Err(TemplateRefusal::EndsItsSegment {
            inside: inside.to_owned(),
        });
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

/// Literal template text as a request spells it: `{{` is `{`, `}}` is `}`.
pub(crate) fn unescape(literal: &str) -> Cow<'_, str> {
    match literal.contains(['{', '}']) {
        false => Cow::Borrowed(literal),
        true => Cow::Owned(literal.replace("{{", "{").replace("}}", "}")),
    }
}

/// Where a template segment's parameter opens, and whether it is the
/// catch-all; `None` for a literal segment. `{{` is literal text.
pub(crate) fn segment_parameter(segment: &str) -> Option<(usize, bool)> {
    let bytes = segment.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'{' if bytes.get(at + 1) == Some(&b'{') => at += 2,
            b'{' => return Some((at, bytes.get(at + 1) == Some(&b'*'))),
            _ => at += 1,
        }
    }
    None
}

/// What is left of `text` once the literal template text `literal` is
/// stripped from its front, reading `{{` as `{`; `None` when it does not
/// begin with it. Runs per request, so it does not allocate.
pub(crate) fn strip_literal<'a>(text: &'a str, literal: &str) -> Option<&'a str> {
    let (text_bytes, literal) = (text.as_bytes(), literal.as_bytes());
    let (mut at, mut read) = (0, 0);
    while read < literal.len() {
        let byte = literal[read];
        read += match byte {
            b'{' | b'}' if literal.get(read + 1) == Some(&byte) => 2,
            _ => 1,
        };
        if text_bytes.get(at) != Some(&byte) {
            return None;
        }
        at += 1;
    }
    text.get(at..)
}

/// A template [`RouteTemplate::parse`] accepted, in poem's spelling: `{name}`
/// is `:name`, `{*name}` is `*name`, `{{` is `{`.
///
/// A translation, not a check: what is not a template comes out as mechanical
/// as it went in, so it is handed only what the grammar read.
pub fn poem_pattern(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find(['{', '}']) {
        out.push_str(&rest[..at]);
        let brace = rest.as_bytes()[at];
        let after = &rest[at + 1..];
        if after.as_bytes().first() == Some(&brace) {
            out.push(char::from(brace));
            rest = &after[1..];
            continue;
        }
        if brace == b'}' {
            out.push('}');
            rest = after;
            continue;
        }
        let close = after.find('}').unwrap_or(after.len());
        match after[..close].strip_prefix('*') {
            Some(name) => {
                out.push('*');
                out.push_str(name);
            }
            None => {
                out.push(':');
                out.push_str(&after[..close]);
            }
        }
        rest = after.get(close + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// The pattern poem's router matched, in the spelling it was declared in:
/// `/users/:id` is `/users/{id}`. A pattern with nothing to translate is
/// handed back as it is. A pattern poem's grammar alone spells (a
/// regular-expression capture an endpoint handed to `HttpTransport::mount`
/// routes on) keeps its expression's text.
pub(crate) fn declared(pattern: &str) -> Cow<'_, str> {
    if !pattern.contains([':', '*', '{', '}']) {
        return Cow::Borrowed(pattern);
    }
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut rest = pattern;
    while let Some(at) = rest.find([':', '*', '{', '}']) {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        match rest.as_bytes()[at] {
            b':' => {
                let len = after.find(['/', '<', '*']).unwrap_or(after.len());
                out.push('{');
                out.push_str(&after[..len]);
                out.push('}');
                rest = &after[len..];
                // A capture's expression has no spelling here: the name stands for it.
                if let Some(expression) = rest.strip_prefix('<') {
                    rest = expression
                        .find('>')
                        .map_or("", |close| &expression[close + 1..]);
                }
            }
            b'*' => {
                out.push_str("{*");
                out.push_str(after);
                out.push('}');
                rest = "";
            }
            brace => {
                out.push(char::from(brace));
                out.push(char::from(brace));
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Templates both parsers read, then templates both refuse.
    const CORPUS: &[&str] = &[
        "/",
        "",
        "//",
        "users",
        "/users/",
        "/a//b",
        "/users/{id}",
        "/users/{id}/",
        "users/{id}/posts/{post_id}",
        "/@{handle}",
        "/report-{id}",
        "/files/{*rest}",
        "/files/x{*rest}",
        "/b/{{x}}",
        "/b/{{{x}",
        "/é/{nom}",
        "/users/:id",
        "/users/:id/x/*rest",
        "/@:handle",
        "/items/:",
        "/files/*",
        "/files/*rest",
        "/n/:id<\\d+>",
        "/n/<\\d+>",
        "/n/{id:\\d+}",
        "/n/{id<\\d+>}",
        "/users/{id}.json",
        "/a/{x}{y}",
        "/files/{*rest}/meta",
        "/files/{*rest}.gz",
        "/a/{}",
        "/a/{*}",
        "/a/{id",
        "/a/{i{d}",
        "/a/b}",
        "/a/b}}",
    ];

    #[test]
    fn codegen_and_runtime_parse_one_corpus_alike() {
        for written in CORPUS {
            match (
                nest_rs_codegen::RoutePath::parse(written),
                RouteTemplate::parse(written),
            ) {
                (Ok(compiled), Ok(booted)) => {
                    assert_eq!(compiled.template(), booted.as_str(), "{written}");
                    assert_eq!(compiled.identity(), booted.identity(), "{written}");
                }
                (Err(compiled), Err(booted)) => {
                    assert_eq!(compiled, booted.to_string(), "{written}");
                }
                (compiled, booted) => {
                    panic!("the two parsers disagree on `{written}`: {compiled:?} / {booted:?}")
                }
            }
        }
    }

    #[test]
    fn codegen_and_runtime_refuse_a_literal_mount_alike() {
        for written in ["/ws/{room}", "/a{{b", "/graphql/:x", "/files/*rest"] {
            let booted = RouteTemplate::literal(written).expect_err("not literal");
            assert_eq!(
                nest_rs_codegen::MountPath::Literal.refusal(written),
                Some(
                    booted
                        .to_string()
                        .replacen(&format!("`{written}`: "), "", 1)
                ),
                "{written}",
            );
        }
        assert_eq!(
            nest_rs_codegen::MountPath::Literal.refusal("/graphql"),
            None
        );
    }

    fn template(written: &str) -> RouteTemplate {
        RouteTemplate::parse(written).unwrap_or_else(|why| panic!("{why}"))
    }

    #[test]
    fn the_poem_spelling_round_trips() {
        for (ours, poems) in [
            ("/{id}", "/:id"),
            ("/@{handle}", "/@:handle"),
            ("/{*rest}", "/*rest"),
            ("/users/{id}/posts/{post_id}", "/users/:id/posts/:post_id"),
            ("/files/x{*rest}", "/files/x*rest"),
            ("/b/{{x}}", "/b/{x}"),
            ("/plain", "/plain"),
            ("/", "/"),
        ] {
            assert_eq!(poem_pattern(template(ours).as_str()), poems, "{ours}");
            assert_eq!(declared(poems), ours, "{poems}");
        }
    }

    #[test]
    fn a_pattern_with_nothing_to_translate_is_handed_back() {
        assert!(matches!(declared("/users/me"), Cow::Borrowed("/users/me")));
    }

    #[test]
    fn a_capture_only_poem_spells_keeps_its_name() {
        assert_eq!(declared("/n/:id<\\d+>/x"), "/n/{id}/x");
        assert_eq!(declared("/f/*"), "/f/{*}");
    }

    #[test]
    fn the_names_are_read_off_the_template() {
        let read = template("/orgs/{org}/@{member}/{*path}");
        assert_eq!(read.parameters().collect::<Vec<_>>(), ["org", "member"]);
        assert_eq!(read.catch_all(), Some("path"));
        assert_eq!(read.identity(), "/orgs/{}/@{}/{*}");
        assert_eq!(template("/plain").catch_all(), None);
    }

    #[test]
    fn a_literal_mount_holds_no_template_syntax() {
        assert_eq!(template_of_literal("graphql/"), "/graphql");
        for written in [
            "/ws/{room}",
            "/a{{b",
            "/graphql/:x",
            "/files/*rest",
            "/n/<x>",
        ] {
            let refused = RouteTemplate::literal(written).expect_err("not literal");
            assert!(
                refused.to_string().contains("one literal address"),
                "{refused}"
            );
        }
    }

    fn template_of_literal(written: &str) -> String {
        RouteTemplate::literal(written)
            .unwrap_or_else(|why| panic!("{why}"))
            .as_str()
            .to_owned()
    }

    #[test]
    fn a_segment_says_where_its_parameter_opens() {
        assert_eq!(segment_parameter("users"), None);
        assert_eq!(segment_parameter("{id}"), Some((0, false)));
        assert_eq!(segment_parameter("@{handle}"), Some((1, false)));
        assert_eq!(segment_parameter("{*rest}"), Some((0, true)));
        assert_eq!(segment_parameter("{{x}}"), None);
        assert_eq!(segment_parameter("{{{x}"), Some((2, false)));
    }

    #[test]
    fn literal_text_is_matched_as_a_request_spells_it() {
        assert_eq!(strip_literal("@bob", "@"), Some("bob"));
        assert_eq!(strip_literal("bob", "@"), None);
        assert_eq!(strip_literal("{x}", "{{x}}"), Some(""));
        assert_eq!(strip_literal("{{x}}", "{{x}}"), None);
        assert_eq!(unescape("{{x}}"), "{x}");
    }
}
