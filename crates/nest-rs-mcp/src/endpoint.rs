//! The endpoint that serves an MCP handler over streamable HTTP.

use std::borrow::Cow;
use std::sync::Arc;

use http::uri::Authority;
use nest_rs_core::problem::code;
use nest_rs_core::{Container, Correlation, Posture, Problem, current_request_scope};
use nest_rs_http::__private::framework_sentence;
use nest_rs_http::ResponseError as _;
use nest_rs_http::{
    Body, BoxEndpoint, DetachedWork, Endpoint, HeaderValue, Method, Request, Response, Result,
    StatusCode, header, normalize_error_response,
};
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestMethod, ClientJsonRpcMessage, ClientRequest, ConstString, ErrorCode,
    JsonRpcError, ProtocolVersion, RequestId,
};
use rmcp::transport::common::http_header::{HEADER_MCP_PROTOCOL_VERSION, HEADER_SESSION_ID};
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::store::SessionStore;

use crate::McpError;
use crate::ambient::McpAmbient;
use crate::config::McpConfig;
use crate::error::{
    SESSION_NOT_FOUND, is_transport_message, said_by_code, tool_not_found, transport_sentence,
};
use crate::propagate::{OperationUnits, PropagatingHandler, RequestChains};
use crate::registry::MountIndex;
use crate::sessions::{BoundedSessions, Principal, Refused, ReportedStore, say_no_subject};
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

/// Everything one `#[mcp]` mount needs beyond the handler itself: the chains
/// each operation runs, its unit of work, and how the streamable-HTTP server is
/// configured. The transport checks the request's origin and runs its guards
/// in front of it, so an endpoint built from one serves whoever reaches it.
pub struct McpMount {
    chains: RequestChains,
    unit_of_work: Option<Arc<dyn UnitOfWork>>,
    config: McpConfig,
    session_store: Option<Arc<dyn SessionStore>>,
    /// Told and stopped by the HTTP transport; a hand-built [`endpoint`]'s is never
    /// told, so its operations end only with their clients' cancellations.
    detached: DetachedWork,
    /// What the path's hosts declare, read at mount; a hand-built
    /// [`endpoint`]'s handler is asked through `get_tool`.
    index: Option<Arc<MountIndex>>,
    /// Who the endpoint serves, which decides what its sessions are bound to.
    posture: Posture,
}

impl McpMount {
    /// A mount on [`McpConfig::default`], with no app's layers and no unit of
    /// work: a hand-built endpoint's, in a test. It is gated, as an app's
    /// endpoint is until the app opens it, so a caller naming no subject
    /// opens no session.
    pub fn new() -> Self {
        Self {
            chains: RequestChains::Bare,
            unit_of_work: None,
            config: McpConfig::default(),
            session_store: None,
            detached: DetachedWork::new(),
            index: None,
            posture: Posture::Gated,
        }
    }

    /// [`new`](Self::new), opened as `McpOptions::posture = Posture::Public`
    /// opens an app's endpoint: no guard runs in front of it, so its sessions
    /// are the anonymous caller's.
    pub fn public() -> Self {
        Self {
            posture: Posture::Public,
            ..Self::new()
        }
    }

    /// Resolve the whole mount from the app's container:
    ///
    /// * the chains every operation's own site runs, which the boot's wiring
    ///   step composed;
    /// * the bound `dyn UnitOfWork` each request operation runs its unit of work
    ///   through — without one a `Repo`-backed handler fails **closed**;
    /// * [`McpConfig`], if `McpModule` was imported, else its defaults;
    /// * a registered `dyn SessionStore` (rmcp 3.x cross-instance session
    ///   recovery), if the app provides one;
    /// * the posture the app declared, `McpOptions::posture`.
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

        let chains = match crate::operation::REQUEST.chains(container) {
            Ok(chains) => RequestChains::Composed(chains),
            Err(refused) => {
                tracing::error!(
                    target: crate::TARGET,
                    error = %nest_rs_core::error_message(refused.as_ref()),
                    "the mount's operations did not compose; every operation is refused",
                );
                RequestChains::Refused
            }
        };

        Self {
            chains,
            unit_of_work: container.get_dyn::<dyn UnitOfWork>(),
            config,
            session_store: container.get_dyn::<dyn SessionStore>(),
            detached: DetachedWork::new(),
            index: None,
            posture: crate::module::posture(container),
        }
    }

    /// Run this mount's operations as `work`, which the HTTP transport stops
    /// when it stops serving — what the `#[mcp]` registration declares on its
    /// [`HttpEndpointMeta`](nest_rs_http::HttpEndpointMeta).
    pub(crate) fn stopped_with(mut self, work: DetachedWork) -> Self {
        self.detached = work;
        self
    }

    /// Name a tool an operation calls through `index`, what the mount read
    /// its hosts declare.
    pub(crate) fn declaring(mut self, index: Arc<MountIndex>) -> Self {
        self.index = Some(index);
        self
    }
}

impl Default for McpMount {
    fn default() -> Self {
        Self::new()
    }
}

/// Mount `factory`'s handler as an endpoint, wrapped per `mount`; the
/// transport runs the request's origin check and guards in front of it.
///
/// `factory` runs on every new MCP session, so per-session state stays fresh.
pub fn endpoint<F, H>(mount: McpMount, factory: F) -> BoxEndpoint
where
    F: Fn() -> H + Send + Sync + 'static,
    H: ServerHandler + Send + 'static,
{
    let McpMount {
        chains,
        unit_of_work,
        config,
        session_store,
        detached,
        index,
        posture,
    } = mount;

    let units = OperationUnits::new(
        unit_of_work,
        detached.clone(),
        config.operation_timeout,
        index.clone(),
        chains,
    );
    let undeclared = index.map(|index| UndeclaredTools {
        index,
        allowed_hosts: config.allowed_hosts.clone(),
        max_request_body_bytes: config.max_request_body_bytes,
        units: units.clone(),
    });

    let session_store = session_store.map(|store| Arc::new(ReportedStore::new(store, posture)));
    let sessions = Arc::new(BoundedSessions::new(
        config.to_session_config(),
        config.max_concurrent_sessions,
        posture,
        session_store.clone(),
    ));

    // The stop, not the signal: rmcp ends every stream on this token, a POST's
    // answer included. A stateful session's operations miss it; the handler stops those.
    let mut server_config = config
        .to_server_config()
        .with_cancellation_token(detached.cancellation_token());
    server_config.session_store = session_store.map(|store| store as Arc<dyn SessionStore>);

    let service = StreamableHttpService::new(
        move || Ok(PropagatingHandler::new(factory(), units.clone())),
        sessions.clone(),
        server_config,
    );
    BoxEndpoint::new(SessionEndpoint {
        session_cap: SessionCap {
            sessions,
            legacy: config.legacy_session_mode,
            max_request_body_bytes: config.max_request_body_bytes,
        },
        undeclared,
        service,
    })
}

/// The streamable-HTTP service behind the session cap, each request carrying
/// what its operations re-install on rmcp's own tasks.
struct SessionEndpoint<H: ServerHandler> {
    session_cap: SessionCap,
    /// `None` on a hand-built [`endpoint`], whose mount read no declarations.
    undeclared: Option<UndeclaredTools>,
    service: StreamableHttpService<PropagatingHandler<H>, BoundedSessions>,
}

/// rmcp's answer to a session it does not hold, word for word: what a request
/// naming another principal's session reads too, so neither tells a client
/// whether the other exists.
fn unknown_session() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(SESSION_NOT_FOUND)
}

/// Why a client whose credential names no subject is refused a session, in
/// words that quote nothing it sent.
const NO_SUBJECT: &str = "a session is bound to the subject of the credential that opens it, and \
                          this credential names none";

/// The answer to a session refused because the caller's principal names no
/// subject: `403`, saying why, and no session id.
fn no_subject() -> Response {
    Problem::new(403, code::FORBIDDEN)
        .with_detail(NO_SUBJECT)
        .as_response()
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
    /// the body read and handed back otherwise. A `caller` no session could
    /// be bound to reads that refusal instead, never told to retry.
    ///
    /// Only a full mount reads the body, and only of a `POST` naming no session:
    /// an `initialize` is the one message that opens a session, and a stateless
    /// request is served at the cap. A creation racing past this check, or a
    /// session restored from the app's store past the cap, is refused by
    /// [`BoundedSessions`] itself, and answered [`refused`](Self::refused) too.
    async fn refusal(&self, req: &mut Request, caller: &Principal) -> Option<Response> {
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
        if !self.sessions.binds(caller) {
            say_no_subject();
            return Some(no_subject());
        }
        self.sessions.say_full();
        Some(Self::refused())
    }

    /// The answer to a session refused at the cap: `503`, and the wait the
    /// client takes before it tries again.
    fn refused() -> Response {
        Problem::new(503, code::UNAVAILABLE)
            .with_retry_after(SESSION_RETRY_AFTER_SECS)
            .as_response()
    }
}

/// What answers, before rmcp's transport sees it, a `tools/call` naming a tool
/// no host on the mount declares.
///
/// rmcp 3.5 looks up the schema of the tool such a call names whenever its
/// `MCP-Protocol-Version` is at least the standard-headers one, building a
/// whole handler to ask, and keeps the answer — a miss included — in a map
/// nothing bounds (`StreamableHttpService::tool_schema`): a caller naming a
/// new tool per request would grow it for good. The call is answered as
/// rmcp's own handler answers it, `tool not found`, and runs as the operation
/// that handler would have run.
struct UndeclaredTools {
    index: Arc<MountIndex>,
    /// rmcp's DNS-rebinding allowlist: a request it refuses for its `Host`
    /// never reaches the lookup, and is left to rmcp to refuse first.
    allowed_hosts: Vec<String>,
    /// What the body is read to, as rmcp would.
    max_request_body_bytes: usize,
    /// What a call answered on its id runs as.
    units: OperationUnits,
}

impl UndeclaredTools {
    /// The answer to `req`, when it calls a tool no host declares at a
    /// version rmcp would look its schema up at — the body read and handed
    /// back otherwise. A call answered on its id is the operation the
    /// handler would have answered, a unit of `request`.
    async fn refusal(&self, req: &mut Request, request: &Correlation) -> Option<Response> {
        if req.method() != Method::POST
            || !looks_up_schemas(req)
            || !host_admitted(req, &self.allowed_hosts)
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
        let undeclared = undeclared_call(&body, &self.index);
        req.set_body(body);
        let call = undeclared?;
        let refused = match call.id {
            Some(_) => {
                nest_rs_core::with_request_scope(
                    current_request_scope(),
                    request.clone(),
                    self.units.undeclared_call(call.name),
                )
                .await
            }
            None => tool_not_found(),
        };
        Some(refused_call(call.id, refused))
    }
}

/// Whether rmcp 3.5 looks up the schema of the tool `req` calls: its
/// `MCP-Protocol-Version` compares at least the standard-headers version's,
/// as rmcp compares it.
fn looks_up_schemas(req: &Request) -> bool {
    req.headers()
        .get(HEADER_MCP_PROTOCOL_VERSION)
        .and_then(|version| version.to_str().ok())
        .is_some_and(|version| version >= ProtocolVersion::STANDARD_HEADERS.as_str())
}

/// Whether rmcp 3.5's DNS-rebinding check admits `req`, read as it reads it:
/// the `Host` field, else the target's authority, its host lowercased without
/// IPv6 brackets, against each allowed entry's host and the port the entry
/// names, if any; an empty list admits every host that parses.
fn host_admitted(req: &Request, allowed_hosts: &[String]) -> bool {
    let authority = match req.headers().get(header::HOST) {
        Some(host) => host
            .to_str()
            .ok()
            .and_then(|host| Authority::try_from(host).ok()),
        None => req.uri().authority().cloned(),
    };
    let Some(authority) = authority else {
        return false;
    };
    let host = unbracketed(authority.host());
    let port = authority.port_u16();
    let admits = |name: &str, entry_port: Option<u16>| {
        unbracketed(name).eq_ignore_ascii_case(host)
            && entry_port.is_none_or(|entry_port| port == Some(entry_port))
    };
    allowed_hosts.is_empty()
        || allowed_hosts.iter().any(|allowed| {
            let allowed = allowed.trim();
            if allowed.is_empty() {
                return false;
            }
            match Authority::try_from(allowed) {
                Ok(entry) => admits(entry.host(), entry.port_u16()),
                Err(_) => admits(allowed, None),
            }
        })
}

/// A host as rmcp compares it, ASCII case aside: without IPv6 brackets.
fn unbracketed(host: &str) -> &str {
    host.trim_matches('[').trim_matches(']')
}

/// The JSON-RPC request in a body, as far as the schema lookup reads it.
#[derive(serde::Deserialize)]
struct Addressing<'a> {
    /// Whatever it holds: rmcp takes a call whose id it cannot read for a
    /// notification, and looks its tool up all the same.
    id: Option<serde_json::Value>,
    #[serde(borrow)]
    method: Option<Cow<'a, str>>,
    #[serde(borrow)]
    params: Option<CallParams<'a>>,
}

/// A `tools/call`'s params, as far as the schema lookup reads them.
#[derive(serde::Deserialize)]
struct CallParams<'a> {
    #[serde(borrow)]
    name: Option<Cow<'a, str>>,
}

/// A `tools/call` naming a tool no host on the mount declares, as rmcp's
/// schema lookup reads it.
#[derive(Debug, PartialEq)]
struct UndeclaredCall {
    /// The tool it names.
    name: String,
    /// `None` when rmcp reads no id in it.
    id: Option<RequestId>,
}

/// The `tools/call` in `body`, when no host on the mount declares the tool it
/// names; `None` when a host does, one the index could not read might, or
/// rmcp would look no schema up for the body. The id decides nothing: rmcp
/// looks up the tool of a call it reads no id in all the same.
fn undeclared_call(body: &[u8], index: &MountIndex) -> Option<UndeclaredCall> {
    let undeclared = |name: &str| index.tools.declares(name) == Some(false);
    match serde_json::from_slice::<Addressing<'_>>(body) {
        Ok(request) => {
            if request.method.as_deref() != Some(CallToolRequestMethod::VALUE) {
                return None;
            }
            let name = request.params?.name?;
            undeclared(&name).then(|| UndeclaredCall {
                name: name.into_owned(),
                id: request.id.and_then(|id| serde_json::from_value(id).ok()),
            })
        }
        // A repeated key, or a field typed otherwise than here: rmcp may
        // still read a call in it, so it is read as rmcp reads it.
        Err(_) => {
            let (name, id) = looked_up(body)?;
            undeclared(&name).then_some(UndeclaredCall { name, id })
        }
    }
}

/// The tool rmcp 3.5 looks up the schema of for `body`, and the id it reads
/// beside it: `validate_standard_headers`' own reading, through rmcp's types.
fn looked_up(body: &[u8]) -> Option<(String, Option<RequestId>)> {
    let message = serde_json::from_slice::<ClientJsonRpcMessage>(body).ok()?;
    let id = match &message {
        ClientJsonRpcMessage::Request(request)
            if matches!(request.request, ClientRequest::InitializeRequest(_)) =>
        {
            return None;
        }
        ClientJsonRpcMessage::Request(request) => Some(request.id.clone()),
        ClientJsonRpcMessage::Notification(_) => None,
        _ => return None,
    };
    let message = serde_json::to_value(&message).ok()?;
    if message.get("method").and_then(serde_json::Value::as_str)
        != Some(CallToolRequestMethod::VALUE)
    {
        return None;
    }
    let name = message.get("params")?.get("name")?.as_str()?;
    Some((name.to_owned(), id))
}

/// The answer to a call of a tool no host declares, `refused`, on the call's
/// id. A call with no id rmcp reads is input the server cannot accept:
/// refused `400`, its error carrying no id, as the streamable-HTTP transport
/// answers it.
fn refused_call(id: Option<RequestId>, refused: McpError) -> Response {
    let status = if id.is_some() {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    let refusal = JsonRpcError::new(id, refused);
    match serde_json::to_vec(&refusal) {
        Ok(body) => Response::builder()
            .status(status)
            .content_type(HeaderValue::from_static("application/json"))
            .body(body),
        // Unreachable: an id and an error always serialize.
        Err(_) => Problem::new(500, code::INTERNAL).as_response(),
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
/// A refusal in one of rmcp's sentences quoting nothing the request carried
/// keeps it: the message, or the problem's `detail`.
async fn said_by_kind(response: Response) -> Response {
    if response.status().is_success() {
        return response;
    }
    let (mut head, body) = response.into_parts();
    let refusal = body.into_bytes().await.unwrap_or_else(|error| {
        tracing::warn!(
            target: crate::TARGET,
            error = %nest_rs_core::error_message(&error),
            "mcp transport refusal unreadable; answered by its status alone",
        );
        Default::default()
    });
    head.headers.remove(header::CONTENT_LENGTH);
    match jsonrpc_said_by_kind(&refusal, head.status) {
        Some(answer) => Response::from_parts(head, Body::from(answer)),
        None => {
            head.headers.remove(header::CONTENT_TYPE);
            let detail = std::str::from_utf8(&refusal)
                .ok()
                .and_then(transport_sentence)
                .map_or_else(Body::empty, |sentence| Body::from(sentence.to_owned()));
            let refusal = Response::from_parts(head, detail);
            normalize_error_response(framework_sentence(refusal)).await
        }
    }
}

/// `refusal` with its JSON-RPC error said by its code, when it is one: the
/// code's words, else the status's reason phrase — rmcp's own message where
/// it quotes nothing the request carried.
fn jsonrpc_said_by_kind(refusal: &[u8], status: StatusCode) -> Option<Vec<u8>> {
    let mut answer: serde_json::Value = serde_json::from_slice(refusal).ok()?;
    let error = answer.get_mut("error")?.as_object_mut()?;
    let code = i32::try_from(error.get("code")?.as_i64()?).ok()?;
    let kept = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .is_some_and(is_transport_message);
    if !kept {
        let said = said_by_code(ErrorCode(code))
            .or_else(|| status.canonical_reason())
            .unwrap_or("Error");
        error.insert("message".to_owned(), said.into());
    }
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

impl<H> Endpoint for SessionEndpoint<H>
where
    H: ServerHandler + Send + 'static,
{
    async fn call(&self, mut req: Request) -> Result<Response> {
        // Read once: two reads would mint two ids for one operation.
        let correlation = nest_rs_core::__private::current_correlation()
            .unwrap_or_else(|| Correlation::minted(None));
        // Past the request's guards: the principal its session is bound to.
        let caller = Principal::of(&correlation);
        if let Some(refused) = self.session_cap.refusal(&mut req, &caller).await {
            return Ok(refused);
        }
        if let Some(undeclared) = &self.undeclared
            && let Some(refused) = undeclared.refusal(&mut req, &correlation).await
        {
            return Ok(refused);
        }

        // Past the request's guards, so what they and its layers made ambient
        // is captured; rmcp forwards the extensions into every operation's
        // `RequestContext`, on a task of its own which no ambient context
        // reaches.
        req.extensions_mut().insert(McpAmbient::capture());
        let scope = current_request_scope();

        // A `GET` is the session's standalone stream, with no end of its own.
        let standalone = req.method() == Method::GET;

        // Also installed here, for an operation rmcp resolves inline.
        let (response, refused) = BoundedSessions::answering(
            caller,
            nest_rs_core::with_request_scope(
                scope,
                correlation,
                self.service.handle(req.into_http()),
            ),
        )
        .await;
        let response = match refused {
            Some(Refused::Full) => return Ok(SessionCap::refused()),
            Some(Refused::NoSubject) => return Ok(no_subject()),
            Some(Refused::Unknown) => unknown_session(),
            None => Response::from(response),
        };
        // rmcp answers `403` to a `Host` outside the allowlist alone, the
        // endpoint keeping its `Origin` check off.
        if response.status() == StatusCode::FORBIDDEN {
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

    /// The hosts rmcp 3.5 admits against its default loopback list, as it
    /// reads them, and the ones it refuses.
    #[test]
    fn a_host_is_admitted_as_rmcp_admits_it() {
        let loopback = McpConfig::default().allowed_hosts;
        let with_host = |host: &'static str| {
            Request::builder()
                .header(header::HOST, HeaderValue::from_static(host))
                .finish()
        };
        for admitted in [
            "localhost",
            "LOCALHOST:8080",
            "127.0.0.1:3000",
            "[::1]",
            "[::1]:3000",
        ] {
            assert!(host_admitted(&with_host(admitted), &loopback), "{admitted}");
        }
        for refused in [
            "rebound.example",
            "localhost.rebound.example",
            "127.0.0.2",
            "a b",
        ] {
            assert!(!host_admitted(&with_host(refused), &loopback), "{refused}");
        }
        assert!(
            !host_admitted(&Request::builder().finish(), &loopback),
            "no Host and no authority: rmcp refuses it 400"
        );
        let by_authority = Request::builder()
            .uri(nest_rs_http::Uri::from_static("http://localhost:3000/mcp"))
            .finish();
        assert!(host_admitted(&by_authority, &loopback));

        let pinned = vec!["api.example:8443".to_owned(), " ".to_owned()];
        assert!(host_admitted(&with_host("API.example:8443"), &pinned));
        assert!(!host_admitted(&with_host("api.example"), &pinned));
        assert!(!host_admitted(&with_host("api.example:443"), &pinned));
        assert!(host_admitted(&with_host("anything.example"), &[]));
    }

    /// The tool a body calls, as rmcp's lookup reads it, refused only when no
    /// host declares it and every host was read, whatever id it carries.
    #[test]
    fn a_call_is_undeclared_only_when_the_index_can_say_so() {
        let mut read = MountIndex::default();
        read.tools.read(0, ["ping".to_owned()], true);
        let call = |body: &str| undeclared_call(body.as_bytes(), &read);
        let marked = |id: Option<RequestId>| {
            Some(UndeclaredCall {
                name: "marked".to_owned(),
                id,
            })
        };

        assert_eq!(
            call(r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"marked"}}"#),
            marked(Some(RequestId::Number(7))),
        );
        assert_eq!(
            call(
                r#"{"jsonrpc":"2.0","id":"a","method":"tools\/call","params":{"name":"m\u0061rked"}}"#
            ),
            marked(Some(RequestId::String("a".into()))),
            "escapes decoded as rmcp decodes them",
        );
        assert_eq!(
            call(
                r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"ping","name":"marked"}}"#
            ),
            marked(Some(RequestId::Number(7))),
            "a repeated name read as rmcp reads it: the last",
        );
        for unread_id in [
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"marked"}}"#,
            r#"{"jsonrpc":"2.0","id":{},"method":"tools/call","params":{"name":"marked"}}"#,
            r#"{"jsonrpc":"2.0","id":null,"method":"tools/call","params":{"name":"marked"}}"#,
            r#"{"jsonrpc":"2.0","id":1.5,"method":"tools/call","params":{"name":"marked"}}"#,
            r#"{"jsonrpc":"2.0","id":7,"id":8,"method":"tools/call","params":{"name":"marked"}}"#,
        ] {
            assert_eq!(
                call(unread_id),
                marked(None),
                "rmcp reads a notification, and looks its tool up: {unread_id}",
            );
        }
        for passed in [
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"ping"}}"#,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":7}}"#,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{},"params":{"name":"marked"}}"#,
            r#"[{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"marked"}}]"#,
            "not json",
        ] {
            assert_eq!(call(passed), None, "{passed}");
        }

        let mut unread = MountIndex::default();
        unread.tools.read(0, Vec::new(), false);
        assert_eq!(
            undeclared_call(
                br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"marked"}}"#,
                &unread,
            ),
            None,
            "a host the index could not read may serve it",
        );
    }

    /// A session refused for a principal naming no subject reads why, in a
    /// sentence quoting nothing it sent, and carries no session.
    #[tokio::test]
    async fn a_principal_naming_no_subject_is_answered_why_and_no_session() {
        let answer = no_subject();

        assert_eq!(answer.status(), StatusCode::FORBIDDEN);
        assert!(!answer.headers().contains_key(HEADER_SESSION_ID));
        let body = answer.into_body().into_bytes().await.expect("a body");
        let problem: serde_json::Value = serde_json::from_slice(&body).expect("a problem");
        assert_eq!(problem["code"], code::FORBIDDEN.as_str(), "{problem}");
        assert_eq!(problem["detail"], NO_SUBJECT, "{problem}");
    }

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
