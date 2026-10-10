//! The poem endpoint that serves an MCP handler over streamable HTTP.

use std::sync::Arc;

use nest_rs_core::{Container, Correlation, current_request_scope};
use nest_rs_http::DetachedWork;
use poem::endpoint::TowerCompatExt;
use poem::{Endpoint, IntoEndpoint, Request, Response, Result, Route};
use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::session::store::SessionStore;

use crate::config::McpConfig;
use crate::context::{McpAmbient, McpToolContext};
use crate::guard::{FallbackMcpGuard, McpOperationGuard};
use crate::guards::deny_all;
use crate::propagate::PropagatingHandler;

/// The operation guard an MCP mount runs, in preference order: the app's
/// registered `dyn McpOperationGuard` (the authz bridge), else the global guard
/// pool through the seeded [`FallbackMcpGuard`], else deny-all.
pub fn resolve_operation_guard(container: &Container) -> Arc<dyn McpOperationGuard> {
    let (guard, mode) = match container.get_dyn::<dyn McpOperationGuard>() {
        Some(guard) => (guard, "operation_guard"),
        None => match container.get::<FallbackMcpGuard>() {
            Some(fallback) => ((fallback.0)(container), "global_guard_pool"),
            // `deny_all` says the fail-closed posture itself, at `warn`.
            None => return deny_all(),
        },
    };
    tracing::debug!(target: crate::TARGET, mode, "mcp operations gated");
    guard
}

/// Everything one `#[mcp]` mount needs beyond the handler itself: who gates an
/// operation, what ambient state is re-installed around it, and how the
/// streamable-HTTP server is configured.
pub struct McpMount {
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
    config: McpConfig,
    session_store: Option<Arc<dyn SessionStore>>,
    /// Told and stopped by the HTTP transport; a hand-built [`endpoint`]'s is never
    /// told, so its operations end only with their clients' cancellations.
    detached: DetachedWork,
}

impl McpMount {
    /// Fail closed: an MCP endpoint mounted without an explicit
    /// [`McpOperationGuard`] denies every request rather than serving the tool
    /// surface unauthenticated.
    pub fn deny_all() -> Self {
        Self {
            guard: deny_all(),
            context: None,
            config: McpConfig::default(),
            session_store: None,
            detached: DetachedWork::new(),
        }
    }

    /// Resolve the whole mount from the app's container:
    ///
    /// * the operation guard, per [`resolve_operation_guard`];
    /// * the registered `dyn McpToolContext` (the ORM/authz bridge), if any —
    ///   without one a `Repo`-backed handler still fails **closed**;
    /// * [`McpConfig`], if `McpModule` was imported, else its defaults;
    /// * a registered `dyn SessionStore` (rmcp 3.x cross-instance session
    ///   recovery), if the app provides one.
    pub fn from_container(container: &Container) -> Self {
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
            guard: resolve_operation_guard(container),
            context: container.get_dyn::<dyn McpToolContext>(),
            config,
            session_store: container.get_dyn::<dyn SessionStore>(),
            detached: DetachedWork::new(),
        }
    }

    /// Run this mount's operations as `work`, which the HTTP transport stops
    /// when it stops serving — what the `#[mcp]` registration declares on its
    /// [`HttpEndpointMeta`](nest_rs_http::HttpEndpointMeta).
    pub(crate) fn stopped_with(mut self, work: DetachedWork) -> Self {
        self.detached = work;
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
        guard,
        context,
        config,
        session_store,
        detached,
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
                detached.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        server_config,
    );
    let inner = service.compat();
    Route::new().at(
        "/",
        nest_rs_http::matched(GuardedEndpoint {
            guard,
            context,
            inner,
        }),
    )
}

struct GuardedEndpoint<E> {
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
    inner: E,
}

impl<E> Endpoint for GuardedEndpoint<E>
where
    E: Endpoint<Output = Response>,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        self.guard.before(&mut req).await?;

        // Post-guard, while the ambient executor and ability are reachable; rmcp
        // forwards the extensions into every operation's `RequestContext`.
        let scope = current_request_scope();
        // Read once: two reads would mint two ids for one operation.
        let correlation = nest_rs_core::__private::current_correlation()
            .unwrap_or_else(|| Correlation::minted(None));
        let captured = self.context.as_ref().map(|context| context.capture(&req));
        // Post-`before`, so the guard sees the ability its chain attached.
        let guard_captured = self.guard.capture(&req);
        req.extensions_mut().insert(McpAmbient {
            scope: scope.clone(),
            captured,
            guard_captured,
            // The request's own span: the interceptor band is the outermost wrap.
            span: tracing::Span::current(),
            correlation: correlation.clone(),
        });

        // A `GET` is the session's standalone stream, with no end of its own.
        let standalone = req.method() == poem::http::Method::GET;

        // Also installed here, for an operation rmcp resolves inline.
        let mut response =
            nest_rs_core::with_request_scope(scope, correlation, self.inner.call(req)).await?;
        // Ended at the shutdown signal; a POST's stream ends with its answer.
        if standalone && response.status().is_success() {
            response
                .extensions_mut()
                .insert(nest_rs_http::OpenEndedBody);
        }
        Ok(response)
    }
}
