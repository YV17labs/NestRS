use std::borrow::Cow;
use std::sync::Arc;

use nest_rs_core::Container;
use poem::Response;
use poem::Route;
use poem::endpoint::BoxEndpoint;

type MountFn = dyn Fn(&Container, Route) -> Route + Send + Sync;

/// How a self-mounted endpoint relates to the global guard pool.
///
/// Global guards run inside the per-route shaper for `#[controller]` routes
/// (so they read `#[public]` after routing). A self-mounted endpoint has no
/// shaper, so the transport applies the global guard chain at its edge. The
/// default is [`Guarded`](EdgePosture::Guarded): a new self-mount is
/// fail-secure until it opts out.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum EdgePosture {
    /// Run the global guard chain at the HTTP edge; a denial rejects the
    /// request (e.g. a WS upgrade GET — an unauthenticated upgrade is refused).
    #[default]
    Guarded,
    /// Skip the global edge guard — this surface gates **in-band** (GraphQL
    /// per operation, MCP per request) or is intentionally anonymous (the
    /// OpenAPI document / UI). In-band surfaces stay fail-secure through
    /// their own seam: GraphQL falls back to the global guard pool when no
    /// operation guard is registered; MCP denies by default when unwired.
    Exempt,
}

/// Discovery metadata for a self-mounting HTTP endpoint owned by another
/// surface (a GraphQL schema, an MCP streamable-HTTP service). The closure
/// nests one opaque sub-endpoint at its own path; `path` and `label` exist
/// only so the transport can list the mount in its boot-time route log.
pub struct HttpEndpointMeta {
    path: Cow<'static, str>,
    label: Cow<'static, str>,
    posture: EdgePosture,
    mount: Arc<MountFn>,
}

impl HttpEndpointMeta {
    /// `path` and `label` accept either a `&'static str` or an owned `String`
    /// — so a module configured via `for_root` can nest at a runtime path.
    /// Defaults to [`EdgePosture::Guarded`]; call [`Self::exempt`] for a
    /// surface that authenticates in-band or is intentionally public.
    pub fn new<F>(
        path: impl Into<Cow<'static, str>>,
        label: impl Into<Cow<'static, str>>,
        mount: F,
    ) -> Self
    where
        F: Fn(&Container, Route) -> Route + Send + Sync + 'static,
    {
        Self {
            path: path.into(),
            label: label.into(),
            posture: EdgePosture::Guarded,
            mount: Arc::new(mount),
        }
    }

    /// Mark this self-mount [`EdgePosture::Exempt`] — the transport skips the
    /// global edge guard (the surface gates in-band, or is public).
    pub fn exempt(mut self) -> Self {
        self.posture = EdgePosture::Exempt;
        self
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn posture(&self) -> EdgePosture {
        self.posture
    }

    pub fn mount(&self, container: &Container, route: Route) -> Route {
        (self.mount)(container, route)
    }
}

type GuardWrapFn = dyn Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
    + Send
    + Sync;

/// Discovery metadata that wraps a single [`EdgePosture::Guarded`] self-mount
/// with the global guard chain. Provided by `nest-rs-guards`'
/// `use_guards_global` (which can see the `Guard` trait); applied by the HTTP
/// transport, which cannot — the closure keeps this crate free of any guard
/// dependency, the same inversion [`HttpEndpointWrap`](crate::HttpEndpointWrap)
/// uses. Absent when no global guard is registered, in which case guarded
/// self-mounts mount unwrapped.
pub struct SelfMountGuardWrap(Arc<GuardWrapFn>);

impl SelfMountGuardWrap {
    pub fn new<F>(wrap: F) -> Self
    where
        F: Fn(&Container, BoxEndpoint<'static, Response>) -> BoxEndpoint<'static, Response>
            + Send
            + Sync
            + 'static,
    {
        Self(Arc::new(wrap))
    }

    /// Wrap `endpoint` with the global guard chain — a denial rejects the
    /// request at this self-mount's edge.
    pub fn apply(
        &self,
        container: &Container,
        endpoint: BoxEndpoint<'static, Response>,
    ) -> BoxEndpoint<'static, Response> {
        (self.0)(container, endpoint)
    }
}
