//! [`Endpoint`], what answers a request, and the metadata a surface mounts
//! one with.

use std::any::Any;
use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use nest_rs_core::{Container, Handler, Posture};
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

/// Discovery metadata for a self-mounting HTTP endpoint owned by another
/// surface (a GraphQL schema, an MCP streamable-HTTP service). The closure
/// nests one opaque sub-endpoint at its own path.
///
/// Its [`Handler`] declares who may call it. The transport runs the global
/// guard pool at the edge of a [`Gated`](Posture::Gated) one before the
/// surface sees the request; a [`Public`](Posture::Public) one (the OpenAPI
/// document) runs none, and an [`InBand`](Posture::InBand) one (`/graphql`)
/// runs the pool itself, its operations each gated by their own posture.
pub struct HttpEndpointMeta {
    path: Cow<'static, str>,
    also: Vec<Cow<'static, str>>,
    label: Cow<'static, str>,
    owner: Option<Cow<'static, str>>,
    handler: &'static Handler,
    in_band: bool,
    self_guarded: bool,
    detached: Option<DetachedWork>,
    mount: Arc<MountFn>,
}

impl HttpEndpointMeta {
    /// Declare a self-mount at `path`, its posture the one `handler` declares;
    /// `path` and `label` may be owned, read from config. The surface answers
    /// under `path` whole, so it is one literal address: a template fails the
    /// boot, naming the owner.
    pub fn new<F>(
        handler: &'static Handler,
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
            handler,
            in_band: false,
            self_guarded: false,
            detached: None,
            mount: Arc::new(mount),
        }
    }

    /// Every **other** path this surface's mount closure registers, such as
    /// `OpenApiModule`'s `/api-json` beside `/api`.
    ///
    /// Entries are route templates, which the transport reads at boot: a
    /// surface owning a subtree declares it (`/.well-known/thing/{*rest}`);
    /// none is assumed.
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

    /// The dispatch site this surface's requests run as: its posture, and the
    /// metadata a guard reads.
    pub fn handler(&self) -> &'static Handler {
        self.handler
    }

    /// Whether the transport runs the global guard pool at this surface's
    /// edge: a [`Gated`](Posture::Gated) surface that does not run it itself.
    pub fn guarded_at_the_edge(&self) -> bool {
        self.handler.posture() == Posture::Gated && !self.in_band
    }

    /// Whether the self-mount's edge access is **implicit**: guarded at the
    /// edge, declaring no guards of its own, and no global guard pool is
    /// active. The transport warns on these at boot.
    pub fn edge_access_is_implicit(&self, global_guards: bool) -> bool {
        !global_guards && !self.self_guarded && self.guarded_at_the_edge()
    }

    /// Mount this surface onto `route`, resolving its dependencies from
    /// `container`.
    pub fn mount(&self, container: &Container, route: Route) -> Route {
        (self.mount)(container, route)
    }
}

/// `meta` as a [`Gated`](Posture::Gated) surface that runs the global guard
/// pool at its own request, beside guards of its own, rather than at the
/// transport's edge (`/mcp`).
pub fn guards_in_band(mut meta: HttpEndpointMeta) -> HttpEndpointMeta {
    meta.in_band = true;
    meta
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

    struct Mounts;

    static UPGRADE: Handler =
        nest_rs_core::__private::declare_handler(nest_rs_core::__private::HandlerDeclaration {
            unit: crate::unit::REQUEST,
            host: std::any::TypeId::of::<Mounts>(),
            host_name: "Mounts",
            method: "",
            address: "/ws",
            posture: Posture::Gated,
            meta: nest_rs_core::__private::MetaLevels::default,
        });

    static DOCUMENT: Handler =
        nest_rs_core::__private::declare_handler(nest_rs_core::__private::HandlerDeclaration {
            unit: crate::unit::REQUEST,
            host: std::any::TypeId::of::<Mounts>(),
            host_name: "Mounts",
            method: "",
            address: "/api",
            posture: Posture::Public,
            meta: nest_rs_core::__private::MetaLevels::default,
        });

    #[test]
    fn a_gated_edge_is_implicit_only_without_a_global_pool() {
        let m = HttpEndpointMeta::new(&UPGRADE, "/ws", "ws", |_c, r| r);
        assert!(m.guarded_at_the_edge());
        assert!(m.edge_access_is_implicit(false));
        assert!(!m.edge_access_is_implicit(true));
        assert!(
            !m.self_guarded().edge_access_is_implicit(false),
            "a surface binding guards of its own is not implicit",
        );
    }

    #[test]
    fn a_public_or_in_band_edge_is_never_implicit() {
        let public = HttpEndpointMeta::new(&DOCUMENT, "/api", "openapi", |_c, r| r);
        assert!(!public.guarded_at_the_edge());
        assert!(!public.edge_access_is_implicit(false));
        let in_band = guards_in_band(HttpEndpointMeta::new(&UPGRADE, "/mcp", "mcp", |_c, r| r));
        assert!(!in_band.guarded_at_the_edge());
        assert!(!in_band.edge_access_is_implicit(false));
    }
}
