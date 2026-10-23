//! [`Router`]: the route table compiled into a tree of path segments, which
//! hands each request to the address its path reaches — its template noted,
//! each parameter percent-decoded once, the method dispatched — or whole to
//! the fallback site.
//!
//! A path is read segment by segment, a literal before a parameter before a
//! catch-all at each, and the next candidate is tried when a deeper segment
//! fails; neither kind of parameter matches an empty value. What that answers
//! on the wire is pinned by `tests/integration/routes.rs` and its siblings
//! (`.claude/decisions/http-server.md`).

use std::borrow::Cow;
use std::sync::Arc;

use http::{Method, StatusCode};

use crate::allow::{Chosen, HeadAsGet, Methods, head_of};
use crate::endpoint::{BoxEndpoint, Endpoint};
use crate::error::{HttpError, PathParameterError, Result, RouteError};
use crate::fallback::NotAllowed;
use crate::matched::MatchedRoute;
use crate::request::Request;
use crate::response::Response;
use crate::route_template::{segment_parameter, unescape};
use crate::routes::{Address, Dispatch};

/// The route table, compiled: what answers each request it is handed.
pub(crate) struct Router {
    root: Node,
    entries: Box<[Entry]>,
    /// What answers a path no address claims and a method an address does
    /// not serve: the fallback site, its `404` and `405`.
    unrouted: BoxEndpoint,
}

impl Router {
    /// `table` compiled, handing what it does not answer to `unrouted`.
    pub(crate) fn new(table: Vec<Address>, unrouted: BoxEndpoint) -> Result<Self, RouteError> {
        let mut root = Node::default();
        let mut entries = Vec::with_capacity(table.len());
        for address in table {
            let index = entries.len();
            if !root.insert(address.template.as_str(), index) {
                return Err(RouteError::Router {
                    template: address.template.as_str().to_owned(),
                    owner: address.owner.to_string(),
                    reason: "the table holds that address twice".to_owned(),
                });
            }
            entries.push(Entry::new(address));
        }
        root.order();
        Ok(Self {
            root,
            entries: entries.into_boxed_slice(),
            unrouted,
        })
    }
}

impl Endpoint for Router {
    async fn call(&self, mut req: Request) -> Result<Response> {
        let mut values = Vec::new();
        let Some(at) = self.root.find(req.uri().path(), &mut values) else {
            return self.unrouted.call(req).await;
        };
        let entry = &self.entries[at];
        if let Some(slot) = req.extensions().get::<MatchedRoute>() {
            slot.note(&entry.template);
        }
        let params = entry.params(&values)?;
        let (leaf, head) = match &entry.answers {
            Answers::Any(leaf) => (leaf, false),
            Answers::Methods(methods) => match methods.choose(req.method()) {
                Chosen::Entry(leaf) => (leaf, false),
                Chosen::HeadOf(leaf) => {
                    req.head_mut().method = Method::GET;
                    req.extensions_mut().insert(HeadAsGet);
                    (leaf, true)
                }
                Chosen::NotAllowed => {
                    req.extensions_mut()
                        .insert(NotAllowed(methods.allow().clone()));
                    return self.unrouted.call(req).await;
                }
            },
        };
        req.set_route(Arc::clone(&entry.template), params);
        let resp = leaf.call(req).await?;
        Ok(if head { head_of(resp) } else { resp })
    }
}

/// What answers at one address.
enum Answers {
    Any(BoxEndpoint),
    Methods(Methods<BoxEndpoint>),
}

/// One address of the table: the template it was registered under, in
/// nestrs's spelling, its parameters' names in the order the path holds them,
/// and what answers there.
struct Entry {
    template: Arc<str>,
    names: Box<[Arc<str>]>,
    answers: Answers,
}

impl Entry {
    fn new(address: Address) -> Self {
        let names = address
            .template
            .parameters()
            .chain(address.template.catch_all())
            .map(Arc::from)
            .collect();
        let answers = match address.dispatch {
            Dispatch::Any(leaf) => Answers::Any(leaf),
            Dispatch::Methods(entries) => Answers::Methods(Methods::new(entries)),
        };
        Self {
            template: Arc::from(address.template.as_str()),
            names,
            answers,
        }
    }

    /// Each parameter by name, percent-decoded once — `%2F` is a `/` inside
    /// the value; one whose decoded bytes are not UTF-8 is the client's `400`,
    /// said with its name and never its value.
    fn params(&self, values: &[&str]) -> Result<Vec<(Arc<str>, String)>> {
        self.names
            .iter()
            .zip(values)
            .map(|(name, raw)| match percent_decoded(raw) {
                Some(value) => Ok((Arc::clone(name), value.into_owned())),
                None => Err(HttpError::new(
                    PathParameterError {
                        name: Arc::clone(name),
                    },
                    StatusCode::BAD_REQUEST,
                )),
            })
            .collect()
    }
}

/// One point of the tree, a segment boundary: what the next segment may be.
#[derive(Default)]
struct Node {
    /// Whole literal segments as a request spells them, sorted.
    literals: Vec<(Box<str>, Node)>,
    /// Parameters, each after the literal text its segment opens with: the
    /// longest text first, a bare parameter last.
    parameters: Vec<(Box<str>, Node)>,
    /// Catch-alls, ordered as `parameters`, each the entry it ends.
    rests: Vec<(Box<str>, usize)>,
    /// The entry whose template ends at this point.
    entry: Option<usize>,
}

impl Node {
    /// File `template`'s segments, ending at `entry`; `false` when an entry
    /// already ends there.
    fn insert(&mut self, template: &str, entry: usize) -> bool {
        let mut node = self;
        for segment in template.split('/').skip(1) {
            node = match segment_parameter(segment) {
                None => child(&mut node.literals, unescape(segment)),
                Some((at, false)) => child(&mut node.parameters, unescape(&segment[..at])),
                Some((at, true)) => {
                    let prefix = unescape(&segment[..at]);
                    if node.rests.iter().any(|(held, _)| **held == *prefix) {
                        return false;
                    }
                    node.rests.push((prefix.into(), entry));
                    return true;
                }
            };
        }
        node.entry.replace(entry).is_none()
    }

    /// Sort each point's candidates into the order a path tries them.
    fn order(&mut self) {
        self.literals.sort_by(|(a, _), (b, _)| a.cmp(b));
        self.parameters
            .sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        self.rests
            .sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        for (_, node) in self.literals.iter_mut().chain(&mut self.parameters) {
            node.order();
        }
    }

    /// The entry `path` reaches, each parameter's raw value pushed onto
    /// `values` in the order the path holds them.
    fn find<'p>(&self, path: &'p str, values: &mut Vec<&'p str>) -> Option<usize> {
        self.below(path.strip_prefix('/')?, values)
    }

    /// The entry `path`, what follows a `/`, reaches from this point.
    fn below<'p>(&self, path: &'p str, values: &mut Vec<&'p str>) -> Option<usize> {
        let (segment, after) = match path.split_once('/') {
            Some((segment, after)) => (segment, Some(after)),
            None => (path, None),
        };
        if let Ok(at) = self
            .literals
            .binary_search_by(|(literal, _)| (**literal).cmp(segment))
            && let Some(hit) = self.literals[at].1.next(after, values)
        {
            return Some(hit);
        }
        for (prefix, node) in &self.parameters {
            if let Some(value) = segment.strip_prefix(&**prefix)
                && !value.is_empty()
            {
                values.push(value);
                if let Some(hit) = node.next(after, values) {
                    return Some(hit);
                }
                values.pop();
            }
        }
        self.rests.iter().find_map(|(prefix, entry)| {
            let value = path
                .strip_prefix(&**prefix)
                .filter(|value| !value.is_empty())?;
            values.push(value);
            Some(*entry)
        })
    }

    /// The entry reached from this point once a segment matched: the rest of
    /// the path's, or this point's own when the path ended.
    fn next<'p>(&self, after: Option<&'p str>, values: &mut Vec<&'p str>) -> Option<usize> {
        match after {
            Some(after) => self.below(after, values),
            None => self.entry,
        }
    }
}

/// The point after `key` among `children`, made when it is not there yet.
fn child<'a>(children: &'a mut Vec<(Box<str>, Node)>, key: Cow<'_, str>) -> &'a mut Node {
    let at = match children.iter().position(|(held, _)| **held == *key) {
        Some(at) => at,
        None => {
            children.push((key.into(), Node::default()));
            children.len() - 1
        }
    };
    &mut children[at].1
}

/// `raw` with each `%XX` escape decoded once, an escape that is not one left
/// as written; `None` when the bytes are not UTF-8.
fn percent_decoded(raw: &str) -> Option<Cow<'_, str>> {
    if !raw.contains('%') {
        return Some(Cow::Borrowed(raw));
    }
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        let hex = |at: usize| bytes.get(at).and_then(|b| char::from(*b).to_digit(16));
        match (byte, hex(at + 1), hex(at + 2)) {
            (b'%', Some(high), Some(low)) => {
                // Two hex digits make one byte: `high * 16 + low` < 256.
                decoded.push(u8::try_from(high * 16 + low).unwrap_or(byte));
                at += 3;
            }
            _ => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(decoded).ok().map(Cow::Owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RouteTemplate;

    /// The tree `templates` compile into, each entry its index.
    fn tree(templates: &[&str]) -> Node {
        let mut root = Node::default();
        for (index, template) in templates.iter().enumerate() {
            let template = RouteTemplate::parse(template).unwrap();
            assert!(root.insert(template.as_str(), index), "{template}");
        }
        root.order();
        root
    }

    /// The template `path` reaches among `templates`, and its raw values.
    fn reached<'p>(
        templates: &[&'static str],
        path: &'p str,
    ) -> Option<(&'static str, Vec<&'p str>)> {
        let mut values = Vec::new();
        tree(templates)
            .find(path, &mut values)
            .map(|at| (templates[at], values))
    }

    #[test]
    fn a_literal_wins_over_a_parameter_and_a_parameter_over_a_catch_all() {
        let table = ["/files/{*rest}", "/files/{name}", "/files/new"];
        assert_eq!(reached(&table, "/files/new"), Some(("/files/new", vec![])));
        assert_eq!(
            reached(&table, "/files/a.txt"),
            Some(("/files/{name}", vec!["a.txt"]))
        );
        assert_eq!(
            reached(&table, "/files/a/b.txt"),
            Some(("/files/{*rest}", vec!["a/b.txt"]))
        );
    }

    /// A literal that leads nowhere gives way to the parameter beside it, and
    /// a parameter to the catch-all beside it.
    #[test]
    fn a_deeper_miss_tries_the_next_candidate() {
        let table = ["/a/b/c", "/a/{x}/d", "/a/{*rest}"];
        assert_eq!(reached(&table, "/a/b/d"), Some(("/a/{x}/d", vec!["b"])));
        assert_eq!(reached(&table, "/a/b/e"), Some(("/a/{*rest}", vec!["b/e"])));
        assert_eq!(reached(&table, "/a/b/c"), Some(("/a/b/c", vec![])));
    }

    /// The first segment where two templates part decides: a literal there
    /// wins over a parameter, whatever follows.
    #[test]
    fn the_first_segment_two_templates_part_at_decides() {
        let table = ["/a/{*rest}", "/{x}/b"];
        assert_eq!(reached(&table, "/a/b"), Some(("/a/{*rest}", vec!["b"])));
        assert_eq!(reached(&table, "/c/b"), Some(("/{x}/b", vec!["c"])));
    }

    #[test]
    fn a_parameter_after_literal_text_wins_over_a_bare_one_and_the_longest_text_first() {
        let table = ["/{id}", "/@{handle}", "/@@{team}", "/@me"];
        assert_eq!(reached(&table, "/@me"), Some(("/@me", vec![])));
        assert_eq!(reached(&table, "/@bob"), Some(("/@{handle}", vec!["bob"])));
        assert_eq!(reached(&table, "/@@ops"), Some(("/@@{team}", vec!["ops"])));
        assert_eq!(reached(&table, "/@"), Some(("/{id}", vec!["@"])));
        assert_eq!(reached(&table, "/bob"), Some(("/{id}", vec!["bob"])));
    }

    #[test]
    fn neither_parameter_matches_an_empty_value() {
        let table = ["/users/{id}", "/a/{x}/b", "/assets/{*rest}", "/{*all}"];
        assert_eq!(
            reached(&table, "/users/"),
            Some(("/{*all}", vec!["users/"]))
        );
        assert_eq!(reached(&table, "/a//b"), Some(("/{*all}", vec!["a//b"])));
        assert_eq!(reached(&table[..3], "/assets/"), None);
        assert_eq!(reached(&table, "/"), None);
    }

    #[test]
    fn a_catch_all_takes_the_rest_after_its_literal_text() {
        let table = ["/files/x{*rest}", "/files/{*rest}"];
        assert_eq!(
            reached(&table, "/files/xa/b"),
            Some(("/files/x{*rest}", vec!["a/b"]))
        );
        assert_eq!(
            reached(&table, "/files/ya/b"),
            Some(("/files/{*rest}", vec!["ya/b"]))
        );
        assert_eq!(
            reached(&table, "/files/x"),
            Some(("/files/{*rest}", vec!["x"]))
        );
    }

    #[test]
    fn the_root_and_an_escaped_brace_are_literal_segments() {
        let table = ["/", "/b/{{x}}", "/b/{y}"];
        assert_eq!(reached(&table, "/"), Some(("/", vec![])));
        assert_eq!(reached(&table, "/b/{x}"), Some(("/b/{{x}}", vec![])));
        assert_eq!(reached(&table, "/b/z"), Some(("/b/{y}", vec!["z"])));
        assert_eq!(reached(&table, "*"), None, "a path that is no path");
    }

    /// The table merges two spellings of one address before it compiles: one
    /// reaching the tree twice is refused, never answered by the second.
    #[test]
    fn an_address_filed_twice_is_refused() {
        for (first, second) in [("/a/{x}", "/a/{y}"), ("/a/{*x}", "/a/{*y}"), ("/a", "/a")] {
            let mut root = Node::default();
            assert!(root.insert(first, 0));
            assert!(!root.insert(second, 1), "{second}");
        }
    }

    #[test]
    fn a_value_is_percent_decoded_once_and_an_escape_that_is_none_kept() {
        assert_eq!(percent_decoded("a%2Fb").as_deref(), Some("a/b"));
        assert_eq!(
            percent_decoded("caf%C3%A9%252F").as_deref(),
            Some("café%2F")
        );
        assert_eq!(percent_decoded("100%").as_deref(), Some("100%"));
        assert_eq!(percent_decoded("%zz%4").as_deref(), Some("%zz%4"));
        assert!(matches!(
            percent_decoded("plain"),
            Some(Cow::Borrowed("plain"))
        ));
        assert_eq!(percent_decoded("%FF%FE"), None);
    }

    /// The entry the router reaches records its template in nestrs's
    /// spelling, each parameter by name, and notes the template for the edge;
    /// a path no entry claims, and a method the address does not serve, reach
    /// the fallback whole.
    #[tokio::test]
    async fn an_entry_records_the_template_it_was_registered_under() {
        let echo = crate::endpoint_fn(|req: Request| async move {
            Ok(Response::builder().body(format!(
                "{} {} {}",
                req.route().unwrap_or("-"),
                req.path_param("org").unwrap_or("-"),
                req.path_param("rest").unwrap_or("-"),
            )))
        });
        let unrouted = BoxEndpoint::new(crate::endpoint_fn(|req: Request| async move {
            let allowed = req
                .extensions()
                .get::<NotAllowed>()
                .map(|allow| allow.0.clone());
            Ok(Response::builder()
                .status(StatusCode::IM_A_TEAPOT)
                .body(match allowed {
                    Some(allow) => format!("unrouted {} allow {allow:?}", req.uri().path()),
                    None => format!("unrouted {}", req.uri().path()),
                }))
        }));
        let router = Router::new(
            vec![Address {
                template: RouteTemplate::parse("/orgs/@{org}/files/{*rest}").unwrap(),
                owner: Arc::from("FilesController"),
                dispatch: Dispatch::Methods(vec![(Method::GET, BoxEndpoint::new(echo))]),
            }],
            unrouted,
        )
        .unwrap();

        let slot = MatchedRoute::default();
        let request = Request::builder()
            .uri("/orgs/@acme/files/a%20b/c".parse().unwrap())
            .extension(slot.clone())
            .finish();
        let resp = router.call(request).await.unwrap();
        assert_eq!(
            resp.into_body().into_string().await.unwrap(),
            "/orgs/@{org}/files/{*rest} acme a b/c"
        );
        assert_eq!(slot.route().as_deref(), Some("/orgs/@{org}/files/{*rest}"));

        for (path, answered) in [("/elsewhere", "unrouted /elsewhere"), ("/", "unrouted /")] {
            let missed = router
                .call(Request::builder().uri(path.parse().unwrap()).finish())
                .await
                .expect("the fallback site answers it");
            assert_eq!(missed.status(), StatusCode::IM_A_TEAPOT);
            assert_eq!(missed.into_body().into_string().await.unwrap(), answered);
        }

        let not_allowed = router
            .call(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/orgs/@acme/files/x".parse().unwrap())
                    .finish(),
            )
            .await
            .expect("the fallback site answers it");
        assert_eq!(
            not_allowed.into_body().into_string().await.unwrap(),
            r#"unrouted /orgs/@acme/files/x allow "GET, HEAD""#
        );
    }
}
