//! Integration tests mirroring `src/`. The shutdown tests speak raw HTTP/1.1 on a
//! loopback port: `TestClient` holds no connection for a shutdown window to close.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

/// One host on `$path` holding its tasks in rmcp's `TaskManager`, as the docs
/// point a host to: the tool `$starts` starts one and answers its id.
macro_rules! task_host {
    ($host:ident, $path:literal, $starts:literal) => {
        #[::nest_rs_mcp::mcp(path = $path)]
        #[derive(Clone, Default)]
        struct $host {
            tasks: ::nest_rs_mcp::rmcp::task_manager::TaskManager,
        }

        impl ::nest_rs_mcp::ServerHandler for $host {
            fn get_info(&self) -> ::nest_rs_mcp::ServerConfig {
                ::nest_rs_mcp::ServerConfig::new(
                    ::nest_rs_mcp::ServerCapabilities::builder()
                        .enable_tools()
                        .enable_tasks()
                        .build(),
                )
            }

            async fn list_tools(
                &self,
                _request: Option<::nest_rs_mcp::model::PaginatedRequestParams>,
                _context: ::nest_rs_mcp::service::RequestContext<
                    ::nest_rs_mcp::service::RoleServer,
                >,
            ) -> Result<::nest_rs_mcp::model::ListToolsResult, ::nest_rs_mcp::McpError> {
                Ok(::nest_rs_mcp::model::ListToolsResult::with_all_items(vec![
                    ::nest_rs_mcp::model::Tool::new(
                        $starts,
                        "Start a task.",
                        ::std::sync::Arc::new(::nest_rs_mcp::model::JsonObject::new()),
                    ),
                ]))
            }

            async fn call_tool(
                &self,
                request: ::nest_rs_mcp::model::CallToolRequestParams,
                _context: ::nest_rs_mcp::service::RequestContext<
                    ::nest_rs_mcp::service::RoleServer,
                >,
            ) -> Result<::nest_rs_mcp::model::CallToolResponse, ::nest_rs_mcp::McpError> {
                if request.name != $starts {
                    return Err(::nest_rs_mcp::McpError::invalid_params(
                        "tool not found",
                        None,
                    ));
                }
                let task = self.tasks.spawn(
                    ::nest_rs_mcp::rmcp::task_manager::TaskOptions::new(),
                    |_| {
                        Box::pin(async {
                            Ok(::nest_rs_mcp::CallToolResult::success(vec![
                                ::nest_rs_mcp::ContentBlock::text("done"),
                            ]))
                        })
                    },
                );
                Ok(
                    ::nest_rs_mcp::CallToolResult::success(vec![
                        ::nest_rs_mcp::ContentBlock::text(task.task_id),
                    ])
                    .into(),
                )
            }

            async fn get_task(
                &self,
                request: ::nest_rs_mcp::model::GetTaskParams,
                _context: ::nest_rs_mcp::service::RequestContext<
                    ::nest_rs_mcp::service::RoleServer,
                >,
            ) -> Result<::nest_rs_mcp::model::GetTaskResult, ::nest_rs_mcp::McpError> {
                Ok(::nest_rs_mcp::model::GetTaskResult::new(
                    self.tasks.get_task(&request.task_id)?,
                ))
            }

            async fn update_task(
                &self,
                request: ::nest_rs_mcp::model::UpdateTaskParams,
                _context: ::nest_rs_mcp::service::RequestContext<
                    ::nest_rs_mcp::service::RoleServer,
                >,
            ) -> Result<(), ::nest_rs_mcp::McpError> {
                self.tasks
                    .update_task(&request.task_id, request.input_responses)
            }

            async fn cancel_task(
                &self,
                request: ::nest_rs_mcp::model::CancelTaskParams,
                _context: ::nest_rs_mcp::service::RequestContext<
                    ::nest_rs_mcp::service::RoleServer,
                >,
            ) -> Result<(), ::nest_rs_mcp::McpError> {
                self.tasks.cancel_task(&request.task_id)
            }
        }
    };
}

mod ambient;
mod endpoint;
mod error;
mod guard;
mod mcp_impl;
mod operation;
mod parameters;
mod propagate;
mod registry;
mod scope;
mod sessions;
mod unit;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_core::{App, Layer, Module, Transport, UnitContext, injectable};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs_http::{HttpConfig, HttpTransport};
use nest_rs_mcp::rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The MCP module with its endpoints opened in code, for a suite whose subject
/// is not who may call them.
pub(crate) fn open_mcp() -> nest_rs_mcp::McpSetup {
    nest_rs_mcp::McpModule::for_root(nest_rs_mcp::McpOptions {
        posture: nest_rs_core::Posture::Public,
        ..Default::default()
    })
}

/// A session on `path` declaring the tasks extension, which `tasks/*`
/// requires, opened as `bearer` says or anonymously.
pub(crate) async fn tasks_session(
    app: &nest_rs_testing::TestApp,
    path: &str,
    bearer: Option<&str>,
) -> String {
    nest_rs_testing::mcp::open_session_with(
        app.http(),
        path,
        bearer,
        &[],
        serde_json::json!({ "extensions": { "io.modelcontextprotocol/tasks": {} } }),
    )
    .await
}

/// What a store refusing the connection reports: the cause a guard keeps.
#[derive(Debug)]
struct ConnectionRefused;

impl std::fmt::Display for ConnectionRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the identity store refused the connection")
    }
}

impl std::error::Error for ConnectionRefused {}

/// The `error` field a [`StoreDown`] denial files: its reason, then its cause.
const STORE_DOWN_CHAIN: &str =
    "the identity store did not answer: the identity store refused the connection";

/// Cannot evaluate a request or an operation: its identity store is down.
#[injectable]
#[derive(Default)]
struct StoreDown;

impl StoreDown {
    fn denial() -> Denial {
        Denial::unavailable_because(None, "the identity store did not answer", ConnectionRefused)
    }
}

impl Layer for StoreDown {}

#[async_trait]
impl Guard for StoreDown {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Self::denial())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Self::denial())
    }
}

impl GuardFor<nest_rs_http::HttpView> for StoreDown {}
impl GuardFor<nest_rs_mcp::McpView> for StoreDown {}

/// An app's session store in memory, shared by the instances a test boots
/// around it as a deployment's are.
#[derive(Default)]
pub(crate) struct MemoryStore {
    states: Mutex<HashMap<String, SessionState>>,
    loads: AtomicUsize,
}

impl MemoryStore {
    /// Whether it holds `session`.
    pub(crate) fn holds(&self, session: &str) -> bool {
        self.states.lock().unwrap().contains_key(session)
    }

    /// How many loads it has answered.
    pub(crate) fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    /// Hold `session` as a release before the binding stored it: the client's
    /// handshake alone.
    pub(crate) fn put_unbound(&self, session: &str) {
        self.put(session, None);
    }

    /// Hold `session` as a release binding it to `principal` stored it: under
    /// the `_meta` key the MCP sessions page names.
    pub(crate) fn put_bound(&self, session: &str, principal: serde_json::Value) {
        self.put(
            session,
            Some(serde_json::json!({ "dev.nestrs/principal": principal })),
        );
    }

    /// Hold `session` as the client's handshake, carrying `meta` as its
    /// `_meta` when there is one.
    fn put(&self, session: &str, meta: Option<serde_json::Value>) {
        let mut params = serde_json::json!({
            "protocolVersion": nest_rs_testing::mcp::PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "elsewhere", "version": "0" },
        });
        if let Some(meta) = meta {
            params["_meta"] = meta;
        }
        let state = serde_json::from_value(serde_json::json!({ "initialize_params": params }))
            .expect("a session's state");
        self.states
            .lock()
            .unwrap()
            .insert(session.to_owned(), state);
    }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn load(&self, session: &str) -> Result<Option<SessionState>, SessionStoreError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.states.lock().unwrap().get(session).cloned())
    }

    async fn store(&self, session: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        self.states
            .lock()
            .unwrap()
            .insert(session.to_owned(), state.clone());
        Ok(())
    }

    async fn delete(&self, session: &str) -> Result<(), SessionStoreError> {
        self.states.lock().unwrap().remove(session);
        Ok(())
    }
}

/// `M` served on a free loopback port by the transport `HttpModule` would build,
/// with `window` as its shutdown window.
pub(crate) struct Serving {
    pub(crate) port: u16,
    cancel: CancellationToken,
    task: JoinHandle<nest_rs_core::anyhow::Result<()>>,
}

pub(crate) async fn serve_on_loopback<M: Module + 'static>(window: Duration) -> Serving {
    serve_under::<M>(HttpConfig {
        shutdown_timeout: window,
        ..HttpConfig::default()
    })
    .await
}

/// `M` served on a free loopback port by a transport built from `config`.
pub(crate) async fn serve_under<M: Module + 'static>(config: HttpConfig) -> Serving {
    let app = App::builder()
        .module::<M>()
        .build()
        .await
        .expect("the module boots");
    let mut transport = HttpTransport::from_config(&HttpConfig {
        host: "127.0.0.1".into(),
        port: 0,
        ..config
    })
    .expect("the config builds a transport");
    transport
        .configure(app.container())
        .await
        .expect("the transport configures");
    let port = transport
        .listen()
        .await
        .expect("the transport listens")
        .addresses
        .first()
        .expect("one loopback address")
        .port();
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move { Box::new(transport).serve(cancel).await }
    });
    Serving { port, cancel, task }
}

impl Serving {
    /// Ask for shutdown with the clock paused — nothing reads a socket
    /// meanwhile — and return how long `serve` took to come back on that clock.
    pub(crate) async fn stop(self) -> Duration {
        tokio::time::pause();
        let asked = tokio::time::Instant::now();
        self.cancel.cancel();
        self.task
            .await
            .expect("serve does not panic")
            .expect("serve stops cleanly");
        let took = asked.elapsed();
        tokio::time::resume();
        took
    }
}

/// A fresh connection to the port the transport listens on.
async fn connect(port: u16) -> TcpStream {
    TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|error| panic!("the transport listens on port {port}: {error}"))
}

/// Write `request` on a fresh connection and read the response head, leaving
/// the connection — and any stream it carries — open.
pub(crate) async fn exchange(port: u16, request: &str) -> (TcpStream, String) {
    let mut stream = connect(port).await;
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request is sent");
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).await.expect("the head is readable");
        assert!(read > 0, "the connection ended inside the head");
        head.push(byte[0]);
    }
    (stream, String::from_utf8_lossy(&head).into_owned())
}

fn session_header(session: Option<&str>) -> String {
    session
        .map(|id| format!("mcp-session-id: {id}\r\n"))
        .unwrap_or_default()
}

/// POST one JSON-RPC message to `path` over raw HTTP/1.1.
pub(crate) async fn post(
    port: u16,
    path: &str,
    session: Option<&str>,
    body: &serde_json::Value,
) -> (TcpStream, String) {
    let body = body.to_string();
    let session = session_header(session);
    exchange(
        port,
        &format!(
            "POST {path} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
             accept: application/json, text/event-stream\r\n{session}content-length: {}\r\n\r\n{body}",
            body.len(),
        ),
    )
    .await
}

/// Open a stateful session on `path` — `initialize`, then
/// `notifications/initialized` — and return its id.
pub(crate) async fn open_raw_session(port: u16, path: &str) -> String {
    let (_, head) = post(
        port,
        path,
        None,
        &nest_rs_testing::mcp::initialize_request(),
    )
    .await;
    let session = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("mcp-session-id: ")
                .map(|_| line["mcp-session-id: ".len()..].trim().to_owned())
        })
        .unwrap_or_else(|| panic!("initialize opens a session: {head}"));
    post(
        port,
        path,
        Some(&session),
        &serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    session
}

/// Open the session's standalone `GET` stream — what a client holds to hear
/// what the server pushes.
pub(crate) async fn get_stream(port: u16, path: &str, session: &str) -> (TcpStream, String) {
    let session = session_header(Some(session));
    exchange(
        port,
        &format!(
            "GET {path} HTTP/1.1\r\nhost: localhost\r\naccept: text/event-stream\r\n{session}\r\n"
        ),
    )
    .await
}

/// Everything left on the socket, up to its end.
pub(crate) async fn read_to_end(stream: &mut TcpStream) -> String {
    let mut rest = Vec::new();
    match tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        Ok(Err(error)) => panic!("the socket failed rather than ending: {error}"),
        Err(_) => panic!("the server never closed the connection"),
    }
    String::from_utf8_lossy(&rest).into_owned()
}
