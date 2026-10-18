//! The poem endpoint that serves an MCP handler over streamable HTTP.

use std::sync::Arc;

use nest_rs_core::problem::code;
use nest_rs_core::{Container, Correlation, Problem, current_request_scope};
use nest_rs_http::__private::attached_pending;
use nest_rs_http::__private::poem_bridge::{
    request_from_poem, request_to_poem, response_from_poem, response_to_poem,
};
use nest_rs_http::ResponseError as _;
use nest_rs_http::{
    Body, DetachedWork, Method, Request, StatusCode, header, normalize_error_response,
};
use poem::endpoint::TowerCompatExt;
use poem::{Endpoint, IntoEndpoint, Response, Result};
use rmcp::ServerHandler;
use rmcp::model::{ClientJsonRpcMessage, ClientRequest, ErrorCode};
use rmcp::transport::common::http_header::HEADER_SESSION_ID;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::store::SessionStore;

use crate::config::McpConfig;
use crate::context::{McpAmbient, McpToolContext};
use crate::error::said_by_code;
use crate::propagate::PropagatingHandler;
use crate::registry::ToolIndex;
use crate::sessions::{BoundedSessions, ReportedStore};
use nest_rs_database::UnitOfWork;

/// The wait, in seconds, a client refused a session is told to take before
/// trying again.
const SESSION_RETRY_AFTER_SECS: u32 = 5;

// rmcp 3.5 quotes what a client sent at every level, `error` included: the
// session id a client named, when the session store fails to load it. The
// mount's sessions and store say its failure lines instead (`sessions.rs`).
nest_rs_core::inventory::submit! {
    nest_rs_core::__private::LibraryClamp::new("rmcp", tracing::level_filters::LevelFilter::OFF)
}

/// Everything one `#[mcp]` mount needs beyond the handler itself: what ambient
/// state is re-installed around an operation, and how the streamable-HTTP
/// server is configured. The transport checks the request's origin and runs its
/// guards in front of it, so an endpoint built from one serves whoever reaches
/// it.
pub struct McpMount {
    context: Option<Arc<dyn McpToolContext>>,
    unit_of_work: Option<Arc<dyn UnitOfWork>>,
    config: McpConfig,
    session_store: Option<Arc<dyn SessionStore>>,
    /// Told and stopped by the HTTP transport; a hand-built [`endpoint`]'s is never
    /// told, so its operations end only with their clients' cancellations.
    detached: DetachedWork,
    /// The tools the path's hosts declare, read at mount; a hand-built
    /// [`endpoint`]'s handler is asked through `get_tool`.
    tools: Option<Arc<ToolIndex>>,
}

impl McpMount {
    /// A mount on [`McpConfig::default`], with no context and no unit of work:
    /// a hand-built endpoint's, in a test.
    pub fn new() -> Self {
        Self {
            context: None,
            unit_of_work: None,
            config: McpConfig::default(),
            session_store: None,
            detached: DetachedWork::new(),
            tools: None,
        }
    }

    /// Resolve the whole mount from the app's container:
    ///
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
            // `debug`: the loopback default is right for a dev run, and each
            // refusal is said at `warn` (`host_refused`).
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
            context: container.get_dyn::<dyn McpToolContext>(),
            unit_of_work: container.get_dyn::<dyn UnitOfWork>(),
            config,
            session_store: container.get_dyn::<dyn SessionStore>(),
            detached: DetachedWork::new(),
            tools: None,
        }
    }

    /// Run this mount's operations as `work`, which the HTTP transport stops
    /// when it stops serving — what the `#[mcp]` registration declares on its
    /// [`HttpEndpointMeta`](nest_rs_http::HttpEndpointMeta).
    pub(crate) fn stopped_with(mut self, work: DetachedWork) -> Self {
        self.detached = work;
        self
    }

    /// Name a tool an operation calls through `tools`, the index the mount
    /// built of what its hosts declare.
    pub(crate) fn declaring(mut self, tools: Arc<ToolIndex>) -> Self {
        self.tools = Some(tools);
        self
    }
}

impl Default for McpMount {
    fn default() -> Self {
        Self::new()
    }
}

/// Mount `factory`'s handler as a poem endpoint, wrapped per `mount`; the
/// transport runs the request's origin check and guards in front of it.
///
/// `factory` runs on every new MCP session, so per-session state stays fresh.
pub fn endpoint<F, H>(mount: McpMount, factory: F) -> impl IntoEndpoint
where
    F: Fn() -> H + Send + Sync + 'static,
    H: ServerHandler + Send + 'static,
{
    let McpMount {
        context,
        unit_of_work,
        config,
        session_store,
        detached,
        tools,
    } = mount;

    let handler_context = context.clone();
    let operation_timeout = config.operation_timeout;

    let sessions = Arc::new(BoundedSessions::new(
        config.to_session_config(),
        config.max_concurrent_sessions,
    ));

    // The stop, not the signal: rmcp ends every stream on this token, a POST's
    // answer included. A stateful session's operations miss it; the handler stops those.
    let mut server_config = config
        .to_server_config()
        .with_cancellation_token(detached.cancellation_token());
    server_config.session_store =
        session_store.map(|store| Arc::new(ReportedStore(store)) as Arc<dyn SessionStore>);

    let service = StreamableHttpService::new(
        move || {
            Ok(PropagatingHandler::new(
                factory(),
                handler_context.clone(),
                unit_of_work.clone(),
                detached.clone(),
                operation_timeout,
                tools.clone(),
            ))
        },
        sessions.clone(),
        server_config,
    );
    let inner = service.compat();
    SessionEndpoint {
        context,
        session_cap: SessionCap {
            sessions,
            legacy: config.legacy_session_mode,
            max_request_body_bytes: config.max_request_body_bytes,
        },
        inner,
    }
}

/// The streamable-HTTP service behind the session cap, each request carrying
/// what its operations re-install on rmcp's own tasks.
struct SessionEndpoint<E> {
    context: Option<Arc<dyn McpToolContext>>,
    session_cap: SessionCap,
    inner: E,
}

/// What refuses an `initialize` before rmcp opens a session past the cap.
struct SessionCap {
    sessions: Arc<BoundedSessions>,
    /// Off, rmcp opens no session at all.
    legacy: bool,
    /// What the body is read to, as rmcp would.
    max_request_body_bytes: usize,
}

impl SessionCap {
    /// The answer refusing `req`, when it would open a session past the cap —
    /// the body read and handed back otherwise.
    ///
    /// Only a full mount reads the body, and only of a `POST` naming no session:
    /// an `initialize` is the one message that opens a session, and a stateless
    /// request is served at the cap. A creation racing past this check is
    /// refused by [`BoundedSessions`] itself, which rmcp answers `500`.
    async fn refusal(&self, req: &mut Request) -> Option<nest_rs_http::Response> {
        if !self.legacy
            || req.method() != Method::POST
            || req.headers().contains_key(HEADER_SESSION_ID)
            || !self.sessions.is_full().await
        {
            return None;
        }
        let body = match req
            .take_body()
            .into_bytes_limit(self.max_request_body_bytes)
            .await
        {
            Ok(body) => body,
            Err(unread) => return Some(unread.as_response()),
        };
        if !opens_a_session(&body) {
            req.set_body(body);
            return None;
        }
        self.sessions.say_full();
        Some(
            Problem::new(503, code::UNAVAILABLE)
                .with_retry_after(SESSION_RETRY_AFTER_SECS)
                .as_response(),
        )
    }
}

/// The line of a request rmcp refused for its `Host`: rmcp's own quotes the
/// `Host`, and is under its clamp.
fn host_refused() {
    tracing::warn!(
        target: crate::TARGET,
        reason = "host_not_allowed",
        hint = %format!(
            "set {} to this deployment's own hostnames",
            nest_rs_config::var_name("mcp", "ALLOWED_HOSTS"),
        ),
        "mcp request refused: its Host is outside the allowlist",
    );
}

/// rmcp's answer to a request its transport turns away, said by kind.
///
/// Several of rmcp's refusals quote what the request carried — a header's
/// value, the method or name its body named — and its failures their cause.
/// The status stays rmcp's; a JSON-RPC error keeps its id, code and data, and
/// says its code's words; any other body is the status's problem document.
async fn said_by_kind(response: Response) -> Response {
    if response.status().is_success() {
        return response;
    }
    let (mut head, body) = response_from_poem(response).into_parts();
    let refusal = body.into_bytes().await.unwrap_or_else(|error| {
        tracing::warn!(
            target: crate::TARGET,
            error = %nest_rs_core::error_message(&error),
            "mcp transport refusal unreadable; answered by its status alone",
        );
        Default::default()
    });
    head.headers.remove(header::CONTENT_LENGTH);
    let said = match jsonrpc_said_by_kind(&refusal, head.status) {
        Some(answer) => nest_rs_http::Response::from_parts(head, Body::from(answer)),
        None => {
            head.headers.remove(header::CONTENT_TYPE);
            normalize_error_response(nest_rs_http::Response::from_parts(head, Body::empty())).await
        }
    };
    response_to_poem(said)
}

/// `refusal` with its JSON-RPC error said by its code, when it is one: the
/// code's words, else the status's reason phrase.
fn jsonrpc_said_by_kind(refusal: &[u8], status: StatusCode) -> Option<Vec<u8>> {
    let mut answer: serde_json::Value = serde_json::from_slice(refusal).ok()?;
    let error = answer.get_mut("error")?.as_object_mut()?;
    let code = i32::try_from(error.get("code")?.as_i64()?).ok()?;
    let said = said_by_code(ErrorCode(code))
        .or_else(|| status.canonical_reason())
        .unwrap_or("Error");
    error.insert("message".to_owned(), said.into());
    serde_json::to_vec(&answer).ok()
}

/// Whether `body` is an `initialize` request, as rmcp decodes it.
fn opens_a_session(body: &[u8]) -> bool {
    matches!(
        serde_json::from_slice::<ClientJsonRpcMessage>(body),
        Ok(ClientJsonRpcMessage::Request(request))
            if matches!(request.request, ClientRequest::InitializeRequest(_))
    )
}

impl<E> Endpoint for SessionEndpoint<E>
where
    E: Endpoint<Output = Response>,
{
    type Output = Response;

    async fn call(&self, req: poem::Request) -> Result<Self::Output> {
        let mut req = request_from_poem(req);
        if let Some(refused) = self.session_cap.refusal(&mut req).await {
            return Ok(response_to_poem(refused));
        }

        // Post-guard, while the ambient executor and ability are reachable; rmcp
        // forwards the extensions into every operation's `RequestContext`.
        let scope = current_request_scope();
        // Read once: two reads would mint two ids for one operation.
        let correlation = nest_rs_core::__private::current_correlation()
            .unwrap_or_else(|| Correlation::minted(None));
        let captured = self.context.as_ref().map(|context| context.capture(&req));
        // What the request's guards attached, re-installed around each
        // operation on rmcp's own task, which no ambient context reaches.
        let attached = attached_pending(&req);
        req.extensions_mut().insert(McpAmbient {
            scope: scope.clone(),
            captured,
            attached,
            // The request's own span: the interceptor band is the outermost wrap.
            span: tracing::Span::current(),
            correlation: correlation.clone(),
        });

        // A `GET` is the session's standalone stream, with no end of its own.
        let standalone = req.method() == Method::GET;

        // Also installed here, for an operation rmcp resolves inline.
        let response = nest_rs_core::with_request_scope(
            scope,
            correlation,
            self.inner.call(request_to_poem(req)),
        )
        .await?;
        // rmcp answers `403` to a `Host` outside the allowlist alone, the
        // endpoint keeping its `Origin` check off.
        if response.status() == poem::http::StatusCode::FORBIDDEN {
            host_refused();
        }
        let mut response = said_by_kind(response).await;
        // Ended at the shutdown signal; a POST's stream ends with its answer.
        if standalone && response.status().is_success() {
            response
                .extensions_mut()
                .insert(nest_rs_http::OpenEndedBody);
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_initialize_opens_a_session() {
        let initialize = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "client", "version": "0" },
            },
        });
        assert!(opens_a_session(initialize.to_string().as_bytes()));

        let listed = serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        assert!(!opens_a_session(listed.to_string().as_bytes()));
        let initialized =
            serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(!opens_a_session(initialized.to_string().as_bytes()));
        assert!(!opens_a_session(b"{ not json"));
    }

    /// rmcp quotes what a client sent at every level, so one `/mcp` denial
    /// files the framework's line alone: rmcp's `response error` is a `warn`
    /// on `rmcp::service`.
    #[test]
    fn rmcp_writes_nothing_unless_a_filter_names_it() {
        use nest_rs_core::__written_under as written;

        for directive in ["info", "debug", "trace"] {
            assert!(!written!(directive, "rmcp::service", ERROR), "{directive}");
            assert!(!written!(directive, "rmcp::service", WARN), "{directive}");
            assert!(!written!(directive, "rmcp::service", INFO), "{directive}");
            assert!(!written!(directive, "rmcp::service", DEBUG), "{directive}");
            assert!(!written!(directive, "rmcp::service", TRACE), "{directive}");
            assert!(
                !written!(directive, "rmcp::service::server", WARN),
                "{directive}"
            );
            assert!(
                !written!(
                    directive,
                    "rmcp::transport::streamable_http_server::tower",
                    WARN
                ),
                "{directive}"
            );
        }
        assert!(written!("info,rmcp=warn", "rmcp::service", WARN));
        assert!(!written!("info,rmcp=warn", "rmcp::service", INFO));
    }
}
