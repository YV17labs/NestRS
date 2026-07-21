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
#[non_exhaustive]
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

    /// The path this surface self-mounts at (e.g. `/graphql`, `/ws`).
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Human-readable label for the boot mount log.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// This self-mount's edge posture — whether the transport runs the global
    /// guard chain at its edge or leaves it to gate in-band.
    pub fn posture(&self) -> EdgePosture {
        self.posture
    }

    /// The self-mount's edge access decision is **implicit**: it is
    /// [`Guarded`](EdgePosture::Guarded) — so it expects the transport to run
    /// the global guard chain at its edge — but no global guard pool is active,
    /// leaving that chain empty. The HTTP transport warns on these at boot, the
    /// self-mount analog of the controller route's `access_is_implicit`.
    ///
    /// An [`Exempt`](EdgePosture::Exempt) self-mount gates in-band or is
    /// deliberately public (the `#[public]` analog), so it is never implicit. A
    /// gateway's own `#[use_guards]` live inside its opaque mount closure and
    /// are invisible here, so a `true` prompts the developer to confirm the
    /// edge is guarded on purpose — it is not proof the edge is wide open.
    pub fn edge_access_is_implicit(&self, global_guards: bool) -> bool {
        !global_guards && self.posture == EdgePosture::Guarded
    }

    /// Mount this surface onto `route`, resolving its dependencies from
    /// `container`.
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
    /// Wrap a guarded self-mount's endpoint in the global guard chain. Supplied
    /// by `nest-rs-guards` (which can see the `Guard` trait); the closure keeps
    /// this crate free of a guard dependency.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> HttpEndpointMeta {
        HttpEndpointMeta::new("/ws", "ws", |_c, r| r)
    }

    #[test]
    fn a_guarded_edge_is_implicit_only_without_a_global_pool() {
        // Default posture is `Guarded`: it expects the global guard chain at
        // its edge, so with no pool active its access is implicit; with a pool
        // the transport shapes it and it is covered.
        let m = meta();
        assert_eq!(m.posture(), EdgePosture::Guarded);
        assert!(m.edge_access_is_implicit(false));
        assert!(!m.edge_access_is_implicit(true));
    }

    #[test]
    fn an_exempt_edge_is_never_implicit() {
        // `Exempt` gates in-band or is deliberately public (the `#[public]`
        // analog), so it is never flagged regardless of the global pool.
        let m = meta().exempt();
        assert_eq!(m.posture(), EdgePosture::Exempt);
        assert!(!m.edge_access_is_implicit(false));
        assert!(!m.edge_access_is_implicit(true));
    }
}
