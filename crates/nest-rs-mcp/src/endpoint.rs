//! The poem endpoint that serves an MCP handler over streamable HTTP.

use std::sync::Arc;

use nest_rs_core::{Container, Correlation, Handler, current_request_scope};
use nest_rs_http::__private::poem_bridge::{
    error_to_poem, request_from_poem, request_to_poem, response_to_poem,
};
use nest_rs_http::__private::{OriginPolicy, attached_pending};
use nest_rs_http::ResponseError as _;
use nest_rs_http::{DetachedWork, Method, Request};
use poem::endpoint::TowerCompatExt;
use poem::{Endpoint, IntoEndpoint, Response, Result};
use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::session::store::SessionStore;

use crate::config::McpConfig;
use crate::context::{McpAmbient, McpToolContext};
use crate::guard::McpOperationGuard;
use crate::guards::{GlobalPoolMcpGuard, deny_all};
use crate::propagate::PropagatingHandler;
use crate::registry::ENDPOINT;
use nest_rs_database::UnitOfWork;
use nest_rs_guards::__private::global_guards_active;

/// The operation guard an MCP mount runs: the global guard pool, with the
/// app's registered `dyn McpOperationGuard` (the authz bridge) added to it; the
/// registered guard alone when the app lists no pool; else deny-all.
pub fn resolve_operation_guard(container: &Container) -> Arc<dyn McpOperationGuard> {
    let bridge = container.get_dyn::<dyn McpOperationGuard>();
    match (global_guards_active(container), bridge) {
        (true, bridge) => {
            tracing::debug!(
                target: crate::TARGET,
                mode = "global_guard_pool",
                bridge = bridge.is_some(),
                "mcp operations gated",
            );
            Arc::new(GlobalPoolMcpGuard::new(container, bridge))
        }
        (false, Some(bridge)) => {
            tracing::debug!(
                target: crate::TARGET,
                mode = "operation_guard",
                "mcp operations gated",
            );
            bridge
        }
        // `deny_all` says the fail-closed posture itself, at `warn`.
        (false, None) => deny_all(),
    }
}

/// Everything one `#[mcp]` mount needs beyond the handler itself: which
/// browser origins may reach it, who gates an operation, what ambient state is
/// re-installed around it, and how the streamable-HTTP server is configured.
pub struct McpMount {
    origins: OriginPolicy,
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
    unit_of_work: Option<Arc<dyn UnitOfWork>>,
    config: McpConfig,
    session_store: Option<Arc<dyn SessionStore>>,
    /// Told and stopped by the HTTP transport; a hand-built [`endpoint`]'s is never
    /// told, so its operations end only with their clients' cancellations.
    detached: DetachedWork,
    /// The site the endpoint's requests run as.
    site: &'static Handler,
}

impl McpMount {
    /// Fail closed: an MCP endpoint mounted without an explicit
    /// [`McpOperationGuard`] denies every request rather than serving the tool
    /// surface unauthenticated.
    pub fn deny_all() -> Self {
        Self {
            origins: OriginPolicy::default(),
            guard: deny_all(),
            context: None,
            unit_of_work: None,
            config: McpConfig::default(),
            session_store: None,
            detached: DetachedWork::new(),
            site: &ENDPOINT,
        }
    }

    /// Resolve the whole mount from the app's container:
    ///
    /// * the browser origins the HTTP transport admits — its own while no CORS
    ///   policy is set, what that policy lists once one is;
    /// * the operation guard, per [`resolve_operation_guard`];
    /// * the registered `dyn McpToolContext`, if any;
    /// * the bound `dyn UnitOfWork` each request operation runs its unit of work
    ///   through — without one a `Repo`-backed handler fails **closed**;
    /// * [`McpConfig`], if `McpModule` was imported, else its defaults;
    /// * a registered `dyn SessionStore` (rmcp 3.x cross-instance session
    ///   recovery), if the app provides one.
    pub(crate) fn from_container(container: &Container) -> Self {
        let config = container
            .get::<McpConfig>()
            .map(|cfg| (*cfg).clone())
            .unwrap_or_default();

        if config.allowed_hosts.is_empty() {
            // An empty allowlist turns off rmcp's DNS-rebinding defence.
            tracing::warn!(
                target: crate::TARGET,
                reason = "host_validation_disabled",
                "mcp host allowlist is empty — inbound Host headers are not validated",
            );
        } else {
            // rmcp's warning on a rejected Host cannot name the variable; `debug`
            // because the loopback default is right for a dev run.
            tracing::debug!(
                target: crate::TARGET,
                allowed_hosts = ?config.allowed_hosts,
                hint = %format!(
                    "a Host outside this list is refused with 403; set {} to this \
                     deployment's own hostnames",
                    nest_rs_config::var_name("mcp", "ALLOWED_HOSTS"),
                ),
                "mcp host allowlist resolved",
            );
        }

        Self {
            origins: OriginPolicy::from_container(container),
            guard: resolve_operation_guard(container),
            context: container.get_dyn::<dyn McpToolContext>(),
            unit_of_work: container.get_dyn::<dyn UnitOfWork>(),
            config,
            session_store: container.get_dyn::<dyn SessionStore>(),
            detached: DetachedWork::new(),
            site: &ENDPOINT,
        }
    }

    /// Run this mount's operations as `work`, which the HTTP transport stops
    /// when it stops serving — what the `#[mcp]` registration declares on its
    /// [`HttpEndpointMeta`](nest_rs_http::HttpEndpointMeta).
    pub(crate) fn stopped_with(mut self, work: DetachedWork) -> Self {
        self.detached = work;
        self
    }

    /// Run the endpoint's requests as `site`, the one `#[mcp]` declared for its
    /// path.
    pub(crate) fn running_as(mut self, site: &'static Handler) -> Self {
        self.site = site;
        self
    }

    /// Replace the operation guard — `AllowAllMcpGuard` for a deliberately
    /// public endpoint, or a test double.
    pub fn with_guard(mut self, guard: Arc<dyn McpOperationGuard>) -> Self {
        self.guard = guard;
        self
    }
}

/// Mount `factory`'s handler as a poem endpoint, gated and wrapped per `mount`.
///
/// `factory` runs on every new MCP session, so per-session state stays fresh.
pub fn endpoint<F, H>(mount: McpMount, factory: F) -> impl IntoEndpoint
where
    F: Fn() -> H + Send + Sync + 'static,
    H: ServerHandler + Send + 'static,
{
    let McpMount {
        origins,
        guard,
        context,
        unit_of_work,
        config,
        session_store,
        detached,
        site,
    } = mount;

    let handler_context = context.clone();
    let handler_guard = guard.clone();

    // The stop, not the signal: rmcp ends every stream on this token, a POST's
    // answer included. A stateful session's operations miss it; the handler stops those.
    let mut server_config = config
        .to_server_config()
        .with_cancellation_token(detached.cancellation_token());
    server_config.session_store = session_store;

    let service = StreamableHttpService::new(
        move || {
            Ok(PropagatingHandler::new(
                factory(),
                handler_guard.clone(),
                handler_context.clone(),
                unit_of_work.clone(),
                detached.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        server_config,
    );
    let inner = service.compat();
    GuardedEndpoint {
        origins,
        guard,
        context,
        site: EndpointSite(site),
        inner,
    }
}

struct GuardedEndpoint<E> {
    origins: OriginPolicy,
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
    site: EndpointSite,
    inner: E,
}

/// The site an endpoint's request runs as, on the request for the guards that
/// run it ([`endpoint_site`]).
#[derive(Clone, Copy)]
struct EndpointSite(&'static Handler);

/// The site the MCP endpoint `req` reached runs its requests as: the one its
/// path declares, [`ENDPOINT`] for a request no endpoint marked.
pub fn endpoint_site(req: &Request) -> &'static Handler {
    req.extensions()
        .get::<EndpointSite>()
        .map_or(&ENDPOINT, |site| site.0)
}

impl<E> Endpoint for GuardedEndpoint<E>
where
    E: Endpoint<Output = Response>,
{
    type Output = Response;

    async fn call(&self, req: poem::Request) -> Result<Self::Output> {
        let mut req = request_from_poem(req);
        if let Err(refused) = self.origins.allows(&req) {
            tracing::warn!(
                target: crate::TARGET,
                origin = refused.origin(),
                expected = refused.expected(),
                "browser origin refused",
            );
            return Ok(response_to_poem(refused.as_response()));
        }
        req.extensions_mut().insert(self.site);
        self.guard.before(&mut req).await.map_err(error_to_poem)?;

        // Post-guard, while the ambient executor and ability are reachable; rmcp
        // forwards the extensions into every operation's `RequestContext`.
        let scope = current_request_scope();
        // Read once: two reads would mint two ids for one operation.
        let correlation = nest_rs_core::__private::current_correlation()
            .unwrap_or_else(|| Correlation::minted(None));
        let captured = self.context.as_ref().map(|context| context.capture(&req));
        // Post-`before`, so the guard sees the ability its chain attached.
        let guard_captured = self.guard.capture(&req);
        // What the request's guards attached, re-installed around each
        // operation on rmcp's own task, which no ambient context reaches.
        let attached = attached_pending(&req);
        req.extensions_mut().insert(McpAmbient {
            scope: scope.clone(),
            captured,
            guard_captured,
            attached,
            // The request's own span: the interceptor band is the outermost wrap.
            span: tracing::Span::current(),
            correlation: correlation.clone(),
        });

        // A `GET` is the session's standalone stream, with no end of its own.
        let standalone = req.method() == Method::GET;

        // Also installed here, for an operation rmcp resolves inline.
        let mut response = nest_rs_core::with_request_scope(
            scope,
            correlation,
            self.inner.call(request_to_poem(req)),
        )
        .await?;
        // Ended at the shutdown signal; a POST's stream ends with its answer.
        if standalone && response.status().is_success() {
            response
                .extensions_mut()
                .insert(nest_rs_http::OpenEndedBody);
        }
        Ok(response)
    }
}
