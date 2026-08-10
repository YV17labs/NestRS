//! The poem endpoint that serves an MCP handler over streamable HTTP.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{Container, current_request_scope};
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
///
/// This is the order [`McpMount::from_container`] resolves with, and the MCP
/// twin of what `ContextEndpoint::new` does for `/graphql`. Keeping deny-all as
/// the tail means the fallback only ever widens what `use_guards_global` opted
/// into — an app with no guards at all still gets a closed tool surface.
pub fn resolve_operation_guard(container: &Container) -> Arc<dyn McpOperationGuard> {
    let (guard, mode) = match container.get_dyn::<dyn McpOperationGuard>() {
        Some(guard) => (guard, "operation_guard"),
        None => match container.get::<FallbackMcpGuard>() {
            Some(fallback) => ((fallback.0)(container), "global_guard_pool"),
            // `deny_all` says the fail-closed posture itself, at `warn`.
            None => return deny_all(),
        },
    };
    tracing::debug!(target: "nest_rs::mcp", mode, "mcp operations gated");
    guard
}

/// Everything one `#[mcp]` mount needs beyond the handler itself: who gates an
/// operation, what ambient state is re-installed around it, and how the
/// streamable-HTTP server is configured.
///
/// Assembled once per mount. [`from_container`](Self::from_container) is what
/// the `#[mcp]` macro emits — the resolution order lives here, in the crate,
/// rather than inside a macro expansion, so it is readable and testable.
pub struct McpMount {
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
    config: McpConfig,
    session_store: Option<Arc<dyn SessionStore>>,
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
            // Not a style nit: an empty allowlist turns off rmcp's DNS-rebinding
            // defence, and this is the event an incident query looks for.
            tracing::warn!(
                target: "nest_rs::mcp",
                reason = "host_validation_disabled",
                "mcp host allowlist is empty — inbound Host headers are not validated",
            );
        } else {
            // rmcp warns when it *rejects* a Host, but its message cannot name
            // the remedy: it knows the allowlist, not that this framework feeds
            // it from `NESTRS_MCP__ALLOWED_HOSTS`. Recording the effective list
            // at mount is what turns that rejection from "why is my deployment
            // answering 403?" into one grep. Deliberately not a `warn`: the
            // loopback default is correct for the local server it protects, and
            // an alarm on every dev run is an alarm nobody reads.
            tracing::debug!(
                target: "nest_rs::mcp",
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
        }
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
    } = mount;

    let handler_context = context.clone();
    let handler_guard = guard.clone();

    let mut server_config = config.to_server_config();
    server_config.session_store = session_store;

    let service = StreamableHttpService::new(
        move || {
            Ok(PropagatingHandler::new(
                factory(),
                handler_guard.clone(),
                handler_context.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        server_config,
    );
    let inner = service.compat();
    Route::new().at(
        "/",
        GuardedEndpoint {
            // What the guard reports it runs is a property of the mount, not of
            // a request — read once here rather than per operation.
            already_ran: Arc::from(guard.already_ran()),
            guard,
            context,
            inner,
        },
    )
}

struct GuardedEndpoint<E> {
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
    already_ran: Arc<[TypeId]>,
    inner: E,
}

impl<E> Endpoint for GuardedEndpoint<E>
where
    E: Endpoint<Output = Response>,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        self.guard.before(&mut req).await?;

        // Capture ambient state here — post-guard, while the request scope and
        // the ambient executor/ability are still reachable — and stash it in
        // the request extensions. rmcp forwards those extensions (as
        // `http::request::Parts`) into every operation's `RequestContext`, so
        // `PropagatingHandler` can re-install them *inside* the spawned
        // dispatch, where a task-local from this task would not reach.
        let scope = current_request_scope();
        let captured = self.context.as_ref().map(|context| context.capture(&req));
        // The guard captures for its own `around` the same way — post-`before`,
        // so it sees the ability its chain just attached.
        let guard_captured = self.guard.capture(&req);
        req.extensions_mut().insert(McpAmbient {
            scope: scope.clone(),
            captured,
            guard_captured,
            already_ran: Some(Arc::clone(&self.already_ran)),
        });

        // Also install the scope here, so an operation rmcp happens to resolve
        // inline (rather than on a spawned task) is covered by the same seam.
        crate::scope::maybe_with_request_scope(scope, self.inner.call(req)).await
    }
}
