//! The router's fallback: the one surface answering what no route and no
//! self-mount claims, outside the global prefix.
//!
//! Which side answers is decided before routing, from the paths the transport
//! mounted, so the request reaches one side whole. The match errs toward a
//! claim — toward the API's own `404` — and never lets the fallback shadow a
//! route.

use std::any::TypeId;
use std::borrow::Cow;
use std::sync::Arc;

use http::header::ALLOW;
use http::{HeaderValue, StatusCode};
use nest_rs_core::__private::{
    HandlerDeclaration, MetaLevels, Site, declare_handler, declare_site,
};
use nest_rs_core::{Container, Handler, Posture};

use crate::endpoint::{BoxEndpoint, Endpoint};
use crate::error::Result;
use crate::matched::MatchedRoute;
use crate::problem::ProblemDetails;
use crate::request::Request;
use crate::response::{IntoResponse, Response};
use crate::route_template::{segment_parameter, unescape};
use crate::site::SiteTail;
use crate::transport::{SELF_MOUNT, join_path, normalize_mount_path};
use crate::versioning::{VersionRefused, route_matches};

/// What the router answers itself — a path no route claims, a method an
/// address does not serve — and the router's fallback: one site, at no
/// address of its own.
pub(crate) static FALLBACK: Handler = declare_handler(HandlerDeclaration {
    unit: crate::unit::REQUEST,
    host: TypeId::of::<crate::HttpModule>(),
    host_name: "HttpModule",
    method: "",
    address: "*",
    posture: Posture::Public,
    meta: MetaLevels::default,
});

/// The fallback's site, folding what a self-mount's does: the unit stage and
/// the app's exception filters around what it answers, so a global catch-all
/// shapes a `404` and a unit-stage interceptor sees it.
pub(crate) static FALLBACK_SITE: Site = declare_site(&FALLBACK, SELF_MOUNT);

/// The methods an address serves, on a request it does not serve with its
/// method: the router answers it through the fallback site.
#[derive(Clone)]
pub(crate) struct NotAllowed(pub(crate) HeaderValue);

/// The router's own answer: `405` with the `Allow` an address serves
/// ([RFC 9110 §15.5.6](https://www.rfc-editor.org/rfc/rfc9110#status.405)),
/// the version selector's refusal, `404` where none claims the path.
pub(crate) struct Unrouted;

impl SiteTail for Unrouted {
    async fn answer(&self, req: &mut Request) -> Result<Response> {
        if let Some(refused) = req.extensions().get::<VersionRefused>() {
            return Ok(refused.answer());
        }
        let Some(NotAllowed(allow)) = req.extensions().get::<NotAllowed>() else {
            return Ok(ProblemDetails::from_status(StatusCode::NOT_FOUND).into_response());
        };
        let mut resp = ProblemDetails::from_status(StatusCode::METHOD_NOT_ALLOWED).into_response();
        resp.headers_mut().insert(ALLOW, allow.clone());
        Ok(resp)
    }
}

type BuildFn = dyn Fn(&Container) -> BoxEndpoint + Send + Sync;

/// Discovery metadata for the router's fallback: a surface answering, at its
/// path and outside the global prefix, every request no route and no
/// self-mount claims — and never one under a path they own.
///
/// It is public by construction: no guard runs at its edge, so registering
/// one is the visible opening. A transport holds one; two fail the boot.
///
/// ```
/// use nest_rs_http::{BoxEndpoint, HttpFallbackMeta, Response, endpoint_fn};
///
/// let fallback = HttpFallbackMeta::new("/", "SpaHost", |_container| {
///     BoxEndpoint::new(endpoint_fn(|_req| async { Ok(Response::builder().body("<app/>")) }))
/// });
/// assert_eq!(fallback.path(), "/");
/// ```
pub struct HttpFallbackMeta {
    path: Cow<'static, str>,
    owner: Cow<'static, str>,
    build: Arc<BuildFn>,
}

impl HttpFallbackMeta {
    /// Declare the fallback at `path`, owned by `owner` (the type a boot error
    /// names); `build` makes the endpoint handed every request it answers,
    /// whole. `path` is one literal address: a template fails the boot.
    pub fn new<F>(
        path: impl Into<Cow<'static, str>>,
        owner: impl Into<Cow<'static, str>>,
        build: F,
    ) -> Self
    where
        F: Fn(&Container) -> BoxEndpoint + Send + Sync + 'static,
    {
        Self {
            path: normalize_mount_path(&path.into()).into(),
            owner: owner.into(),
            build: Arc::new(build),
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

    pub(crate) fn build(&self, container: &Container) -> BoxEndpoint {
        (self.build)(container)
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
    /// The catch-all under its path, which names a request it answers as a
    /// route's template names one the router matched.
    template: Arc<str>,
    claims: Claims,
    endpoint: BoxEndpoint,
}

impl Fallback {
    pub(crate) fn new(path: String, claims: Claims, endpoint: BoxEndpoint) -> Self {
        // A route claimed under a prefix also claimed adds nothing per request.
        let Claims { prefixes, routes } = claims;
        let routes = routes
            .into_iter()
            .filter(|route| !prefixes.iter().any(|(prefix, _)| is_under(route, prefix)))
            .collect();
        Self {
            template: Arc::from(join_path(&path, "/{*path}")),
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

impl<E: Endpoint> Endpoint for WithFallback<E> {
    async fn call(&self, req: Request) -> Result<Response> {
        match &self.fallback {
            Some(fallback) if fallback.answers(req.uri().path()) => {
                if let Some(matched) = req.extensions().get::<MatchedRoute>() {
                    matched.note(&fallback.template);
                }
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
        .take_while(|segment| segment_parameter(segment).is_none())
        .collect();
    normalize_mount_path(&unescape(&literal.join("/")))
}

#[cfg(test)]
mod tests {
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
            BoxEndpoint::new(crate::endpoint_fn(|_| async {
                Ok(Response::builder().body("fallback"))
            })),
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
