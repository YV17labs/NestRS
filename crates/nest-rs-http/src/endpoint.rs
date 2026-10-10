//! [`Endpoint`], what answers a request, and the metadata a surface mounts
//! one with.

use std::any::Any;
use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use nest_rs_core::Container;
use poem::Route;

use crate::detached::DetachedWork;
use crate::error::Result;
use crate::request::Request;
use crate::response::Response;

/// Anything that answers a request: a route, a self-mounted surface, a layer
/// around another endpoint.
///
/// ```
/// use nest_rs_http::{Endpoint, Request, Response, Result};
///
/// struct Hello;
///
/// impl Endpoint for Hello {
///     async fn call(&self, _req: Request) -> Result<Response> {
///         Ok(Response::builder().body("hello"))
///     }
/// }
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
///
/// let response = Hello.call(Request::builder().finish()).await?;
/// assert_eq!(response.into_body().into_string().await?, "hello");
/// # Ok(())
/// # }
/// ```
pub trait Endpoint: Send + Sync + 'static {
    /// Answers `req`.
    fn call(&self, req: Request) -> impl Future<Output = Result<Response>> + Send;
}

/// The object-safe twin [`BoxEndpoint`] erases through.
trait DynEndpoint: Send + Sync + 'static {
    fn call_boxed(&self, req: Request) -> BoxFuture<'_, Result<Response>>;
}

impl<E: Endpoint> DynEndpoint for E {
    fn call_boxed(&self, req: Request) -> BoxFuture<'_, Result<Response>> {
        Box::pin(self.call(req))
    }
}

/// A type-erased endpoint, cloned by one `Arc` count: what a table and a wrap
/// hold.
///
/// ```
/// use nest_rs_http::{BoxEndpoint, Endpoint, Request, Response, endpoint_fn};
///
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let endpoints = [
///     BoxEndpoint::new(endpoint_fn(|_req| async { Ok(Response::builder().body("a")) })),
///     BoxEndpoint::new(endpoint_fn(|_req| async { Ok(Response::builder().body("b")) })),
/// ];
/// let second = endpoints[1].clone().call(Request::builder().finish()).await?;
/// assert_eq!(second.into_body().into_string().await?, "b");
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct BoxEndpoint(Arc<dyn DynEndpoint>);

impl BoxEndpoint {
    /// `endpoint`, erased; one already boxed is kept as it is.
    ///
    /// ```
    /// use nest_rs_http::{BoxEndpoint, Response, endpoint_fn};
    ///
    /// let boxed = BoxEndpoint::new(endpoint_fn(|_req| async { Ok(Response::default()) }));
    /// let again = BoxEndpoint::new(boxed);
    /// ```
    pub fn new(endpoint: impl Endpoint) -> Self {
        if let Some(boxed) = (&endpoint as &dyn Any).downcast_ref::<Self>() {
            return boxed.clone();
        }
        Self(Arc::new(endpoint))
    }
}

impl Endpoint for BoxEndpoint {
    fn call(&self, req: Request) -> impl Future<Output = Result<Response>> + Send {
        self.0.call_boxed(req)
    }
}

impl fmt::Debug for BoxEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxEndpoint").finish_non_exhaustive()
    }
}

/// An endpoint from an async function, as tower's and hyper's `service_fn`.
///
/// ```
/// use nest_rs_http::{Endpoint, Request, Response, StatusCode, endpoint_fn};
///
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let gone = endpoint_fn(|_req: Request| async {
///     Ok(Response::builder().status(StatusCode::GONE).finish())
/// });
/// assert_eq!(gone.call(Request::builder().finish()).await?.status(), StatusCode::GONE);
/// # Ok(())
/// # }
/// ```
pub fn endpoint_fn<F, Fut>(f: F) -> impl Endpoint
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Response>> + Send + 'static,
{
    EndpointFn(f)
}

struct EndpointFn<F>(F);

impl<F, Fut> Endpoint for EndpointFn<F>
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Response>> + Send + 'static,
{
    fn call(&self, req: Request) -> impl Future<Output = Result<Response>> + Send {
        (self.0)(req)
    }
}

/// `endpoint`, with `value` in every request's extensions: the per-route data
/// `#[meta]` and `#[public]` attach.
#[doc(hidden)]
pub fn with_data<E, T>(endpoint: E, value: T) -> impl Endpoint
where
    E: Endpoint,
    T: Clone + Send + Sync + 'static,
{
    WithData { endpoint, value }
}

struct WithData<E, T> {
    endpoint: E,
    value: T,
}

impl<E: Endpoint, T: Clone + Send + Sync + 'static> Endpoint for WithData<E, T> {
    fn call(&self, mut req: Request) -> impl Future<Output = Result<Response>> + Send {
        req.extensions_mut().insert(self.value.clone());
        self.endpoint.call(req)
    }
}

type MountFn = dyn Fn(&Container, Route) -> Route + Send + Sync;

/// How a self-mounted endpoint relates to the global guard pool. A self-mount
/// has no route shaper, so the default, [`Guarded`](EdgePosture::Guarded), runs
/// the global guard chain at its edge.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum EdgePosture {
    /// Run the global guard chain at the HTTP edge; a denial rejects the
    /// request (e.g. a WS upgrade GET — an unauthenticated upgrade is refused).
    #[default]
    Guarded,
    /// Skip the global edge guard — this surface gates **in-band** (GraphQL
    /// per operation, MCP per request) or is intentionally anonymous (the
    /// OpenAPI document / UI), and must stay fail-secure through its own seam.
    Exempt,
}

/// Discovery metadata for a self-mounting HTTP endpoint owned by another
/// surface (a GraphQL schema, an MCP streamable-HTTP service). The closure
/// nests one opaque sub-endpoint at its own path.
pub struct HttpEndpointMeta {
    path: Cow<'static, str>,
    also: Vec<Cow<'static, str>>,
    label: Cow<'static, str>,
    owner: Option<Cow<'static, str>>,
    posture: EdgePosture,
    self_guarded: bool,
    detached: Option<DetachedWork>,
    mount: Arc<MountFn>,
}

impl HttpEndpointMeta {
    /// Declare a self-mount at `path`, [`EdgePosture::Guarded`] until
    /// [`Self::exempt`]; `path` and `label` may be owned, read from config.
    pub fn new<F>(
        path: impl Into<Cow<'static, str>>,
        label: impl Into<Cow<'static, str>>,
        mount: F,
    ) -> Self
    where
        F: Fn(&Container, Route) -> Route + Send + Sync + 'static,
    {
        Self {
            // Canonical before anything compares it, so two spellings of one
            // mount cannot blind the collision check.
            path: crate::normalize_mount_path(&path.into()).into(),
            also: Vec::new(),
            label: label.into(),
            owner: None,
            posture: EdgePosture::Guarded,
            self_guarded: false,
            detached: None,
            mount: Arc::new(mount),
        }
    }

    /// Every **other** path this surface's mount closure registers, such as
    /// `OpenApiModule`'s `/api-json` beside `/api`.
    ///
    /// Entries are poem route patterns: a surface owning a subtree declares it
    /// (`/.well-known/thing/*rest`); none is assumed.
    pub fn also_mounts<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<Cow<'static, str>>,
    {
        self.also.extend(
            paths
                .into_iter()
                .map(|path| Cow::Owned(crate::normalize_mount_path(&path.into()))),
        );
        self
    }

    /// Every path this surface answers at: [`path`](Self::path) first, then
    /// whatever [`also_mounts`](Self::also_mounts) declared.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.path.as_ref()).chain(self.also.iter().map(Cow::as_ref))
    }

    /// Mark this self-mount [`EdgePosture::Exempt`] — the transport skips the
    /// global edge guard (the surface gates in-band, or is public).
    pub fn exempt(mut self) -> Self {
        self.posture = EdgePosture::Exempt;
        self
    }

    /// Name the type that owns this mount (`ChatGateway`, `PostsTools`), which a
    /// collision on one path reports.
    pub fn owned_by(mut self, owner: impl Into<Cow<'static, str>>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    /// Declare that this surface binds its own guards at its edge (a gateway's
    /// `#[use_guards]`), which the transport cannot see inside the closure.
    pub fn self_guarded(self) -> Self {
        self.self_guarded_if(true)
    }

    /// [`self_guarded`](Self::self_guarded) driven by a flag, for a macro.
    pub fn self_guarded_if(mut self, yes: bool) -> Self {
        self.self_guarded = yes;
        self
    }

    /// Declare the work this surface runs off the connections that ask for it
    /// (an MCP operation on rmcp's own task); the transport stops it when it
    /// stops serving. See [`DetachedWork`].
    pub fn runs_detached(mut self, work: DetachedWork) -> Self {
        self.detached = Some(work);
        self
    }

    /// The work this surface declared through
    /// [`runs_detached`](Self::runs_detached), if any.
    pub(crate) fn detached(&self) -> Option<&DetachedWork> {
        self.detached.as_ref()
    }

    /// The path this surface self-mounts at (e.g. `/graphql`, `/ws`).
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Human-readable label for the boot mount log.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The owning type's name when one was declared, else the kind — what a
    /// collision error names.
    pub fn owner(&self) -> &str {
        self.owner.as_deref().unwrap_or(&self.label)
    }

    /// This self-mount's edge posture — whether the transport runs the global
    /// guard chain at its edge or leaves it to gate in-band.
    pub fn posture(&self) -> EdgePosture {
        self.posture
    }

    /// Whether the self-mount's edge access is **implicit**: it is
    /// [`Guarded`](EdgePosture::Guarded), declared no guards of its own, and no
    /// global guard pool is active. The transport warns on these at boot.
    pub fn edge_access_is_implicit(&self, global_guards: bool) -> bool {
        !global_guards && !self.self_guarded && self.posture == EdgePosture::Guarded
    }

    /// Mount this surface onto `route`, resolving its dependencies from
    /// `container`.
    pub fn mount(&self, container: &Container, route: Route) -> Route {
        (self.mount)(container, route)
    }
}

type PoemBoxEndpoint = poem::endpoint::BoxEndpoint<'static, poem::Response>;

type GuardWrapFn = dyn Fn(&Container, PoemBoxEndpoint) -> PoemBoxEndpoint + Send + Sync;

/// Discovery metadata that wraps a single [`EdgePosture::Guarded`] self-mount
/// with the global guard chain, provided by `nest-rs-guards`'
/// `use_guards_global`. Absent when no global guard is registered.
pub struct SelfMountGuardWrap(Arc<GuardWrapFn>);

impl SelfMountGuardWrap {
    /// Wrap a guarded self-mount's endpoint in the global guard chain.
    pub fn new<F>(wrap: F) -> Self
    where
        F: Fn(&Container, PoemBoxEndpoint) -> PoemBoxEndpoint + Send + Sync + 'static,
    {
        Self(Arc::new(wrap))
    }

    /// Wrap `endpoint` with the global guard chain — a denial rejects the
    /// request at this self-mount's edge.
    pub fn apply(&self, container: &Container, endpoint: PoemBoxEndpoint) -> PoemBoxEndpoint {
        (self.0)(container, endpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Public;

    #[tokio::test]
    async fn with_data_hands_the_value_to_every_request() {
        let seen = endpoint_fn(|req: Request| async move {
            let marked = req.extensions().get::<Public>().is_some();
            Ok(Response::builder().body(if marked { "public" } else { "unmarked" }))
        });
        let endpoint = with_data(seen, Public);
        for _ in 0..2 {
            let response = endpoint.call(Request::builder().finish()).await.unwrap();
            assert_eq!(response.into_body().into_string().await.unwrap(), "public");
        }
    }

    #[test]
    fn a_boxed_endpoint_is_kept_as_it_is() {
        let boxed = BoxEndpoint::new(endpoint_fn(|_req| async { Ok(Response::default()) }));
        let again = BoxEndpoint::new(boxed.clone());
        assert!(
            Arc::ptr_eq(&boxed.0, &again.0),
            "no second box around the first"
        );
    }

    #[tokio::test]
    async fn an_endpoints_error_reaches_the_caller() {
        let refusing = BoxEndpoint::new(endpoint_fn(|_req| async {
            Err(crate::HttpError::from_status(http::StatusCode::FORBIDDEN))
        }));
        let refused = refusing
            .call(Request::builder().finish())
            .await
            .unwrap_err();
        assert_eq!(refused.status(), http::StatusCode::FORBIDDEN);
    }

    fn meta() -> HttpEndpointMeta {
        HttpEndpointMeta::new("/ws", "ws", |_c, r| r)
    }

    #[test]
    fn a_guarded_edge_is_implicit_only_without_a_global_pool() {
        let m = meta();
        assert_eq!(m.posture(), EdgePosture::Guarded);
        assert!(m.edge_access_is_implicit(false));
        assert!(!m.edge_access_is_implicit(true));
    }

    #[test]
    fn an_exempt_edge_is_never_implicit() {
        let m = meta().exempt();
        assert_eq!(m.posture(), EdgePosture::Exempt);
        assert!(!m.edge_access_is_implicit(false));
        assert!(!m.edge_access_is_implicit(true));
    }
}
