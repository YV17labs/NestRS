//! The router's fallback: the one surface answering what no route and no
//! self-mount claims, outside the global prefix.
//!
//! Which side answers is decided before routing, from the paths the transport
//! mounted, so the request reaches one side whole. The match errs toward a
//! claim — toward the API's own `404` — and never lets the fallback shadow a
//! route.

use std::borrow::Cow;
use std::sync::Arc;

use nest_rs_core::Container;
use poem::endpoint::BoxEndpoint;
use poem::{Endpoint, Request, Response, Result, Route};

use crate::route_template::unescape;
use crate::transport::{is_pattern_segment, normalize_mount_path};
use crate::versioning::route_matches;

type MountFn = dyn Fn(&Container, Route) -> Route + Send + Sync;

/// Discovery metadata for the router's fallback: a surface answering, at its
/// path and outside the global prefix, every request no route and no
/// self-mount claims — and never one under a path they own.
///
/// It is public by construction: no guard runs at its edge, so registering
/// one is the visible opening. A transport holds one; two fail the boot.
pub struct HttpFallbackMeta {
    path: Cow<'static, str>,
    owner: Cow<'static, str>,
    mount: Arc<MountFn>,
}

impl HttpFallbackMeta {
    /// Declare the fallback at `path`, owned by `owner` (the type a boot error
    /// names); `mount` registers its routes on a router of its own. `path` is
    /// one literal address: a template fails the boot.
    pub fn new<F>(
        path: impl Into<Cow<'static, str>>,
        owner: impl Into<Cow<'static, str>>,
        mount: F,
    ) -> Self
    where
        F: Fn(&Container, Route) -> Route + Send + Sync + 'static,
    {
        Self {
            path: normalize_mount_path(&path.into()).into(),
            owner: owner.into(),
            mount: Arc::new(mount),
        }
    }

    /// The path the fallback answers under, canonical.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The type that owns the fallback.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub(crate) fn mount(&self, container: &Container) -> Route {
        (self.mount)(container, Route::new())
    }
}

/// What the route table claims: the prefixes its owners hold whole, and the
/// patterns of the routes mounted outside them.
#[derive(Default)]
pub(crate) struct Claims {
    prefixes: Vec<(String, String)>,
    routes: Vec<String>,
}

impl Claims {
    /// `owner` answers everything under `pattern`'s literal segments.
    pub(crate) fn prefix(&mut self, pattern: &str, owner: impl Into<String>) {
        self.prefixes.push((literal_prefix(pattern), owner.into()));
    }

    /// A route answers at `pattern`, outside any prefix claimed whole.
    pub(crate) fn route(&mut self, pattern: String) {
        self.routes.push(pattern);
    }

    /// Everything mounts under the global prefix, so it is the one claim.
    pub(crate) fn only(&mut self, prefix: String, owner: String) {
        self.prefixes = vec![(prefix, owner)];
        self.routes.clear();
    }

    /// The prefix holding `path`, and its owner.
    pub(crate) fn owner_of(&self, path: &str) -> Option<(&str, &str)> {
        self.prefixes
            .iter()
            .find(|(prefix, _)| is_under(path, prefix))
            .map(|(prefix, owner)| (prefix.as_str(), owner.as_str()))
    }

    fn claim(&self, path: &str) -> bool {
        self.prefixes
            .iter()
            .any(|(prefix, _)| is_under(path, prefix))
            || self.routes.iter().any(|route| route_matches(path, route))
    }
}

/// The router's fallback, mounted.
pub(crate) struct Fallback {
    path: String,
    claims: Claims,
    endpoint: BoxEndpoint<'static, Response>,
}

impl Fallback {
    pub(crate) fn new(
        path: String,
        claims: Claims,
        endpoint: BoxEndpoint<'static, Response>,
    ) -> Self {
        // A route claimed under a prefix also claimed adds nothing per request.
        let Claims { prefixes, routes } = claims;
        let routes = routes
            .into_iter()
            .filter(|route| !prefixes.iter().any(|(prefix, _)| is_under(route, prefix)))
            .collect();
        Self {
            path,
            claims: Claims { prefixes, routes },
            endpoint,
        }
    }

    fn answers(&self, path: &str) -> bool {
        is_under(path, &self.path) && !self.claims.claim(path)
    }
}

/// The route table, beside the fallback answering what it does not claim.
pub(crate) struct WithFallback<E> {
    routes: E,
    fallback: Option<Fallback>,
}

impl<E> WithFallback<E> {
    pub(crate) fn new(routes: E, fallback: Option<Fallback>) -> Self {
        Self { routes, fallback }
    }
}

impl<E: Endpoint<Output = Response>> Endpoint for WithFallback<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Response> {
        match &self.fallback {
            Some(fallback) if fallback.answers(req.uri().path()) => {
                fallback.endpoint.call(req).await
            }
            _ => self.routes.call(req).await,
        }
    }
}

/// Whether `path` is `prefix` or lies below it, segment by segment.
pub(crate) fn is_under(path: &str, prefix: &str) -> bool {
    prefix == "/"
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The literal segments a route template opens with, as a request spells
/// them: `/api-json/{*version}` owns `/api-json`, `/{org}/members` nothing but
/// `/`.
pub(crate) fn literal_prefix(pattern: &str) -> String {
    let literal: Vec<&str> = pattern
        .split('/')
        .take_while(|segment| !is_pattern_segment(segment))
        .collect();
    normalize_mount_path(&unescape(&literal.join("/")))
}

#[cfg(test)]
mod tests {
    use poem::EndpointExt;

    use super::*;

    #[test]
    fn a_path_is_under_its_prefix_segment_by_segment() {
        assert!(is_under("/api", "/api"));
        assert!(is_under("/api/posts", "/api"));
        assert!(
            !is_under("/apidocs", "/api"),
            "a shared spelling is not a segment"
        );
        assert!(!is_under("/", "/api"));
        assert!(is_under("/anything", "/"));
    }

    #[test]
    fn a_pattern_owns_the_literal_segments_it_opens_with() {
        assert_eq!(literal_prefix("/api-json/{*version}"), "/api-json");
        assert_eq!(literal_prefix("/posts/{id}/edit"), "/posts");
        assert_eq!(literal_prefix("/@{handle}/posts"), "/");
        assert_eq!(literal_prefix("/{org}/members"), "/");
        assert_eq!(literal_prefix("/b/{{x}}/{id}"), "/b/{x}");
        assert_eq!(literal_prefix("/graphql"), "/graphql");
    }

    fn fallback(path: &str, claims: Claims) -> Fallback {
        Fallback::new(
            path.to_owned(),
            claims,
            poem::endpoint::make_sync(|_| "fallback")
                .map_to_response()
                .boxed(),
        )
    }

    #[test]
    fn the_fallback_answers_its_subtree_minus_what_the_table_claims() {
        let mut claims = Claims::default();
        claims.prefix("/api", "the API");
        claims.prefix("/graphql", "graphql");
        claims.route("/".to_owned());
        claims.route("/{slug}/edit".to_owned());
        claims.route("/api/posts".to_owned());
        let root = fallback("/", claims);
        assert!(!root.answers("/"), "a route at `/`");
        assert!(root.answers("/assets/app.js"));
        assert!(!root.answers("/api/typo"), "the API's own 404");
        assert!(!root.answers("/graphql/x"));
        assert!(root.answers("/apidocs"));
        assert!(
            !root.answers("/about/edit"),
            "a capture claims what it may match"
        );
        assert!(root.answers("/about"));
        assert_eq!(
            root.claims.routes.len(),
            2,
            "a route under a claimed prefix is dropped"
        );

        let assets = fallback("/assets", Claims::default());
        assert!(assets.answers("/assets/app.js"));
        assert!(!assets.answers("/other"));
    }

    #[test]
    fn the_global_prefix_is_the_one_claim() {
        let mut claims = Claims::default();
        claims.prefix("/posts", "PostsController");
        claims.route("/".to_owned());
        claims.only("/api".to_owned(), "the global prefix".to_owned());
        let root = fallback("/", claims);
        assert!(root.answers("/"));
        assert!(root.answers("/posts"));
        assert!(!root.answers("/api/posts"));
    }
}
