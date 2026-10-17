//! [`Endpoint`], what answers a request, and the metadata a surface mounts
//! one with.

use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use nest_rs_core::{Container, Handler};
use nest_rs_guards::GuardSpec;

use crate::detached::DetachedWork;
use crate::endpoint_guards::CarriedView;
use crate::error::{OriginRefused, Result};
use crate::request::Request;
use crate::response::Response;
use crate::routes::HttpRoutes;

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

type MountFn = dyn Fn(&Container, &mut HttpRoutes) + Send + Sync;

/// Discovery metadata for a self-mounting HTTP endpoint owned by another
/// surface (a GraphQL schema, an MCP streamable-HTTP service). The closure
/// registers its entries on the transport's [`HttpRoutes`], at its path, below
/// it, or at what [`also_mounts`](Self::also_mounts) declares: a registration
/// anywhere else fails the boot naming both. Each entry sees the request's
/// full URI, the global prefix included.
///
/// Its [`Handler`] declares who may call it, and the transport runs its
/// request's guard stage in front of every entry, before the surface sees the
/// request: none for a [`Public`](nest_rs_core::Posture::Public) one (the
/// OpenAPI document); otherwise the global guard pool, with the surface's own
/// guards (a gateway's) or, for an endpoint whose request carries units of its
/// own ([`carries`](Self::carries)), the [`EndpointGuards`] the app's modules
/// declare for them. An [`InBand`](nest_rs_core::Posture::InBand) one
/// (`/graphql`) lets an authentication guard admit an anonymous request to its
/// operations' own gates; a [`Gated`](nest_rs_core::Posture::Gated) one
/// carrying units, with no guard at all, denies every request.
///
/// [`EndpointGuards`]: crate::EndpointGuards
pub struct HttpEndpointMeta {
    path: Cow<'static, str>,
    also: Vec<Cow<'static, str>>,
    label: Cow<'static, str>,
    owner: Option<Cow<'static, str>>,
    handler: &'static Handler,
    guards: Vec<GuardSpec>,
    carried: Option<Carried>,
    origin: Option<OriginGate>,
    detached: Option<DetachedWork>,
    mount: Arc<MountFn>,
}

/// The units a self-mount's request carries, by their view.
#[derive(Clone, Copy)]
pub(crate) struct Carried {
    /// The view's type, which [`EndpointGuards`](crate::EndpointGuards) names.
    pub(crate) view: TypeId,
    /// The view's name, as a boot error says it.
    pub(crate) name: &'static str,
}

/// The requests of a self-mount on which the transport checks a browser's
/// `Origin` before any guard runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginCheck {
    /// A WebSocket upgrade, which a browser opens without consulting CORS
    /// (RFC 6455 §10.2).
    Upgrades,
    /// Every request: the MCP transport's rule.
    Every,
}

/// A self-mount's origin check, as it declared it.
#[derive(Clone, Copy)]
pub(crate) struct OriginGate {
    /// Which requests it checks.
    pub(crate) on: OriginCheck,
    /// Files a refusal on the surface's own target.
    pub(crate) refused: fn(&OriginRefused),
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
        F: Fn(&Container, &mut HttpRoutes) + Send + Sync + 'static,
    {
        Self {
            // Canonical before anything compares it, so two spellings of one
            // mount cannot blind the collision check.
            path: crate::normalize_mount_path(&path.into()).into(),
            also: Vec::new(),
            label: label.into(),
            owner: None,
            handler,
            guards: Vec::new(),
            carried: None,
            origin: None,
            detached: None,
            mount: Arc::new(mount),
        }
    }

    /// Every path this surface's mount closure registers **outside** its own
    /// path, such as `OpenApiModule`'s `/api-json` beside `/api`.
    ///
    /// Entries are route templates, which the transport reads at boot: a
    /// surface owning another subtree declares it (`/.well-known/thing/{*rest}`);
    /// none is assumed, and a registration neither the path nor any of them
    /// covers fails the boot.
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

    /// Declare that this surface's request carries units whose view is `V` —
    /// GraphQL operations, MCP operations — so the
    /// [`EndpointGuards<V, _>`](crate::EndpointGuards) the app's modules declare
    /// join the pool at its request.
    pub fn carries<V: CarriedView>(mut self) -> Self {
        self.carried = Some(Carried {
            view: TypeId::of::<V>(),
            name: std::any::type_name::<V>(),
        });
        self
    }

    /// The surface's own guards, as [`guarded_by`] declared them.
    pub(crate) fn guards(&self) -> &[GuardSpec] {
        &self.guards
    }

    /// The units its request carries, as [`carries`](Self::carries) declared
    /// them.
    pub(crate) fn carried(&self) -> Option<Carried> {
        self.carried
    }

    /// Its origin check, as [`checks_origin`] declared it.
    pub(crate) fn origin(&self) -> Option<OriginGate> {
        self.origin
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

    /// Register this surface's entries on `routes`, resolving its
    /// dependencies from `container`.
    pub fn mount(&self, container: &Container, routes: &mut HttpRoutes) {
        (self.mount)(container, routes);
    }
}

/// `meta`, its own guards — a gateway's `#[use_guards]`, whose emitter asserts
/// each one checks HTTP requests — run at its request after the global pool, a
/// guard the pool lists running once, at the pool's place.
pub fn guarded_by(mut meta: HttpEndpointMeta, guards: Vec<GuardSpec>) -> HttpEndpointMeta {
    meta.guards = guards;
    meta
}

/// `meta`, its browser `Origin` checked on the requests `on` names before any
/// guard runs, against the transport's policy (`OriginPolicy`); `refused` files
/// a refusal on the surface's own target before the transport answers `403`.
pub fn checks_origin(
    mut meta: HttpEndpointMeta,
    on: OriginCheck,
    refused: fn(&OriginRefused),
) -> HttpEndpointMeta {
    meta.origin = Some(OriginGate { on, refused });
    meta
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Posture;

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

    struct Socket;

    impl nest_rs_core::UnitView for Socket {}

    impl CarriedView for Socket {}

    #[test]
    fn a_self_mount_declares_its_guards_its_carried_units_and_its_origin_check() {
        let bare = HttpEndpointMeta::new(&UPGRADE, "/ws", "ws", |_c, _r| {});
        assert!(bare.guards().is_empty());
        assert!(bare.carried().is_none());
        assert!(bare.origin().is_none());

        let declared = checks_origin(
            HttpEndpointMeta::new(&UPGRADE, "/mcp", "mcp", |_c, _r| {}).carries::<Socket>(),
            OriginCheck::Every,
            |_| {},
        );
        assert_eq!(
            declared.carried().map(|units| units.view),
            Some(TypeId::of::<Socket>())
        );
        assert_eq!(
            declared.origin().map(|gate| gate.on),
            Some(OriginCheck::Every)
        );
        assert_eq!(DOCUMENT.posture(), Posture::Public);
    }
}
