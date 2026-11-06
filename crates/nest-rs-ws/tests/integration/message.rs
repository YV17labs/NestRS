//! A message on the pipeline, over a real upgrade: the gateway's layers and its
//! own, the app's, its request scope, what its upgrade attached, the boot
//! checks its gateway's wiring runs, and its connection hooks' deadlines.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use nest_rs_core::{
    Caught, DuplicateMetaError, ExceptionFilter, GlobalExceptionFilters, GlobalInterceptors,
    Interceptor, Layer, Next, Outcome, Problem, Scoped, UnitContext, WiringFailedError, injectable,
    module,
};
use nest_rs_core::{async_trait, code};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_testing::{LogCapture, TestApp};
use nest_rs_ws::{CloseCode, WsConfig, WsModule, WsReply, WsView, gateway, messages};

/// The messages [`Counting`] wrapped.
static WRAPPED: AtomicUsize = AtomicUsize::new(0);

/// Counts every message it runs around.
#[injectable]
#[derive(Default)]
struct Counting;

impl Layer for Counting {}

#[async_trait]
impl Interceptor for Counting {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        if cx.unit().name() == nest_rs_ws::unit::MESSAGE.name() {
            WRAPPED.fetch_add(1, Ordering::SeqCst);
        }
        next.run(cx).await
    }
}

#[gateway(path = "/counted")]
#[use_interceptors(Counting)]
struct CountedGateway;

#[messages]
impl CountedGateway {
    #[subscribe_message("echo")]
    #[public]
    async fn echo(&self, text: String) -> String {
        text
    }

    /// Declared again beside the message: one pool, so it still runs once.
    #[subscribe_message("again")]
    #[use_interceptors(Counting)]
    #[public]
    async fn again(&self, text: String) -> String {
        text
    }
}

#[module(imports = [WsModule], providers = [Counting, CountedGateway])]
struct CountedModule;

#[tokio::test]
async fn a_gateway_interceptor_wraps_every_message_once() {
    let app = TestApp::builder()
        .module::<CountedModule>()
        .build_ws()
        .await
        .expect("a gateway binding an interceptor boots");
    let before = WRAPPED.load(Ordering::SeqCst);

    let mut socket = app.socket("/counted").connect().await;
    for event in ["echo", "again", "echo"] {
        socket.send(event, serde_json::json!("hi")).await;
        assert_eq!(socket.next_envelope().await["data"], "hi", "{event}");
    }

    assert_eq!(WRAPPED.load(Ordering::SeqCst) - before, 3);
    app.shutdown().await.expect("the transport stops cleanly");
}

/// The messages [`Listed`], the app's own interceptor, ran around.
static LISTED: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct Listed;

impl Layer for Listed {}

#[async_trait]
impl Interceptor for Listed {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        if cx.unit().name() == nest_rs_ws::unit::MESSAGE.name() {
            LISTED.fetch_add(1, Ordering::SeqCst);
        }
        next.run(cx).await
    }
}

#[gateway(path = "/plain")]
struct PlainGateway;

#[messages]
impl PlainGateway {
    #[subscribe_message("echo")]
    #[public]
    async fn echo(&self, text: String) -> String {
        text
    }
}

#[module(imports = [WsModule], providers = [Listed, PlainGateway])]
struct ListedModule;

#[module(imports = [ListedModule], providers = [GlobalInterceptors<(Listed,)>])]
struct TheAppsInterceptorsRunAroundEveryMessageApp;

#[tokio::test]
async fn the_apps_interceptors_run_around_every_message() {
    let app = TestApp::builder()
        .module::<TheAppsInterceptorsRunAroundEveryMessageApp>()
        .build_ws()
        .await
        .expect("an app listing an interceptor boots");
    let before = LISTED.load(Ordering::SeqCst);

    let mut socket = app.socket("/plain").connect().await;
    for _ in 0..2 {
        socket.send("echo", serde_json::json!("hi")).await;
        assert_eq!(socket.next_envelope().await["data"], "hi");
    }

    assert_eq!(LISTED.load(Ordering::SeqCst) - before, 2);
    app.shutdown().await.expect("the transport stops cleanly");
}

/// A domain error that says nothing a client may read.
#[derive(Debug, thiserror::Error)]
#[error("sku-42 is out of stock at the eu-west warehouse")]
struct OutOfStock;

/// Answers [`OutOfStock`] as a conflict.
#[injectable]
#[derive(Default)]
struct StockFilter;

impl Layer for StockFilter {}

#[async_trait]
impl ExceptionFilter for StockFilter {
    type Exception = OutOfStock;

    async fn catch(&self, _: &OutOfStock, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(409, code::CONFLICT).with_detail("try another item"))
    }
}

#[gateway(path = "/stock")]
#[use_exception_filters(StockFilter)]
struct StockGateway;

#[messages]
impl StockGateway {
    #[subscribe_message("buy")]
    #[public]
    async fn buy(&self) -> Result<String, OutOfStock> {
        Err(OutOfStock)
    }
}

#[module(imports = [WsModule], providers = [StockFilter, StockGateway])]
struct StockModule;

/// The handler's own error never reaches the client, and a refusal the filter
/// answered files nothing.
#[tokio::test]
async fn a_filter_answers_a_typed_error_with_a_frame_carrying_its_code() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<StockModule>()
        .build_ws()
        .await
        .expect("a gateway binding an exception filter boots");

    let mut socket = app.socket("/stock").connect().await;
    socket.send("buy", serde_json::Value::Null).await;
    let frame = socket.next_envelope().await;

    assert_eq!(frame["event"], "buy", "{frame}");
    assert_eq!(frame["data"]["error"], "try another item", "{frame}");
    assert_eq!(frame["data"]["code"], "CONFLICT", "{frame}");
    assert!(!frame.to_string().contains("eu-west"), "{frame}");
    app.shutdown().await.expect("the transport stops cleanly");

    let filed: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| event.target == nest_rs_ws::TARGET && event.level != "debug")
        .filter(|event| event.level == "warn" || event.level == "error")
        .collect();
    assert!(filed.is_empty(), "{filed:#?}");
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::MESSAGE.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
        "an answered failure is still a failed unit",
    );
}

/// What a unit-stage interceptor read of the reply once the message ran.
static READ_AT_THE_UNIT_STAGE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The event it read beside it.
static EVENTS_AT_THE_UNIT_STAGE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Reads the reply the client is about to get.
#[nest_rs_core::interceptor(stage = unit)]
#[derive(Default)]
struct ReadsTheReply;

impl Layer for ReadsTheReply {}

#[async_trait]
impl Interceptor for ReadsTheReply {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let outcome = next.run(cx).await;
        if let Some(view) = cx.switch_to::<WsView>() {
            let read = match view.reply() {
                Some(WsReply::Error(frame)) => format!("error: {}", frame.error),
                Some(WsReply::Reply(data)) => format!("reply: {data}"),
                Some(WsReply::None) => "silence".to_owned(),
                Some(other) => format!("{other:?}"),
                None => "nothing".to_owned(),
            };
            EVENTS_AT_THE_UNIT_STAGE
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(view.event().to_owned());
            READ_AT_THE_UNIT_STAGE
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(read);
        }
        outcome
    }
}

/// Refuses every message it checks.
#[injectable]
#[derive(Default)]
struct Closed;

impl Layer for Closed {}

#[async_trait]
impl Guard for Closed {
    async fn check(&self, _: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("closed for the night"))
    }
}

impl GuardFor<WsView> for Closed {}

#[gateway(path = "/read")]
struct ReadGateway;

#[messages]
impl ReadGateway {
    #[subscribe_message("open")]
    #[public]
    async fn open(&self) -> &'static str {
        "welcome"
    }

    #[subscribe_message("closed")]
    #[use_guards(Closed)]
    #[public]
    async fn closed(&self) -> &'static str {
        "unreachable"
    }
}

#[module(imports = [WsModule], providers = [ReadsTheReply, Closed, ReadGateway])]
struct ReadModule;

#[tokio::test]
async fn the_unit_stage_reads_the_reply_a_message_answers_a_denial_included() {
    let app = TestApp::builder()
        .module::<ReadModule>()
        .build_ws()
        .await
        .expect("a module bringing a unit-stage interceptor boots");

    let mut socket = app.socket("/read").connect().await;
    socket.send("open", serde_json::Value::Null).await;
    assert_eq!(socket.next_envelope().await["data"], "welcome");
    socket.send("closed", serde_json::Value::Null).await;
    assert_eq!(
        socket.next_envelope().await["data"]["error"],
        "closed for the night"
    );
    app.shutdown().await.expect("the transport stops cleanly");

    let read = READ_AT_THE_UNIT_STAGE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(
        read,
        [r#"reply: "welcome""#, "error: closed for the night"],
        "{read:?}"
    );
}

/// The request scopes the messages of a gateway ran in, kept alive so two of
/// them compare by address.
type Seen = Mutex<Vec<std::sync::Arc<nest_rs_core::RequestScope>>>;

static SHARED_SEEN: Seen = Mutex::new(Vec::new());
static PER_MESSAGE_SEEN: Seen = Mutex::new(Vec::new());

fn keep_scope(seen: &Seen) {
    if let Some(scope) = nest_rs_core::current_request_scope() {
        seen.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(scope);
    }
}

/// Whether the first two messages ran in one scope.
fn one_scope(seen: &Seen) -> bool {
    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(seen.len(), 2, "each message runs in a request scope");
    std::sync::Arc::ptr_eq(&seen[0], &seen[1])
}

#[gateway(path = "/scope")]
struct ScopeGateway;

#[messages]
impl ScopeGateway {
    #[subscribe_message("where")]
    #[public]
    async fn scope(&self) -> bool {
        keep_scope(&SHARED_SEEN);
        true
    }
}

#[module(imports = [WsModule], providers = [ScopeGateway])]
struct SharedScopeModule;

/// The messages a [`PerMessage`] was built for.
static BUILT: AtomicUsize = AtomicUsize::new(0);

/// Which [`PerMessage`] this is.
struct Ticket(usize);

impl Default for Ticket {
    fn default() -> Self {
        Self(BUILT.fetch_add(1, Ordering::SeqCst))
    }
}

/// Built afresh for each message.
#[injectable(scope = request)]
#[derive(Default)]
struct PerMessage {
    ticket: Ticket,
}

#[gateway(path = "/per-message")]
struct PerMessageGateway;

#[messages]
impl PerMessageGateway {
    #[subscribe_message("which")]
    #[public]
    async fn which(&self, per_message: Scoped<PerMessage>, again: Scoped<PerMessage>) -> bool {
        per_message.ticket.0 == again.ticket.0
    }

    #[subscribe_message("where")]
    #[public]
    async fn scope(&self) -> bool {
        keep_scope(&PER_MESSAGE_SEEN);
        true
    }
}

#[module(imports = [WsModule], providers = [PerMessage, PerMessageGateway])]
struct PerMessageModule;

/// Two messages on one socket, each answered before the next is sent.
async fn two_messages(app: &nest_rs_testing::ws::WsApp, path: &str) {
    let mut socket = app.socket(path).connect().await;
    for _ in 0..2 {
        socket.send("where", serde_json::Value::Null).await;
        assert_eq!(socket.next_envelope().await["data"], true);
    }
}

#[tokio::test]
async fn two_messages_share_one_request_scope_while_nothing_is_request_scoped() {
    let app = TestApp::builder()
        .module::<SharedScopeModule>()
        .build_ws()
        .await
        .expect("the gateway boots");
    two_messages(&app, "/scope").await;
    assert!(
        one_scope(&SHARED_SEEN),
        "nothing to build, so nothing to open"
    );
    app.shutdown().await.expect("the transport stops cleanly");

    let app = TestApp::builder()
        .module::<PerMessageModule>()
        .build_ws()
        .await
        .expect("the gateway boots");
    two_messages(&app, "/per-message").await;
    assert!(
        !one_scope(&PER_MESSAGE_SEEN),
        "a request-scoped provider gets a scope per message"
    );
    let mut socket = app.socket("/per-message").connect().await;
    socket.send("which", serde_json::Value::Null).await;
    assert_eq!(
        socket.next_envelope().await["data"],
        true,
        "two parameters of one message resolve one instance",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

/// Admits every upgrade for tenant `acme`.
#[injectable]
#[derive(Default)]
struct AcmeUpgrade;

impl Layer for AcmeUpgrade {}

#[async_trait]
impl Guard for AcmeUpgrade {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        if let Some(tenant) = nest_rs_core::__private::TenantId::new("acme") {
            cx.attach(tenant);
        }
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AcmeUpgrade {}

#[gateway(path = "/tenant")]
#[use_guards(AcmeUpgrade)]
struct TenantGateway;

#[messages]
impl TenantGateway {
    #[subscribe_message("tenant")]
    #[public]
    async fn tenant(&self) -> Option<String> {
        nest_rs_core::current_tenant_id()
    }
}

#[module(imports = [WsModule], providers = [AcmeUpgrade, TenantGateway])]
struct TenantModule;

#[tokio::test]
async fn the_tenant_the_upgrade_attached_is_every_messages() {
    let app = TestApp::builder()
        .module::<TenantModule>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let mut socket = app.socket("/tenant").connect().await;
    for _ in 0..2 {
        socket.send("tenant", serde_json::Value::Null).await;
        assert_eq!(socket.next_envelope().await["data"], "acme");
    }
    app.shutdown().await.expect("the transport stops cleanly");
}

/// A value whose expression hides its type from the expansion.
struct Tag;

impl Tag {
    fn new() -> Self {
        Self
    }
}

#[gateway(path = "/tagged")]
#[meta(Tag::new())]
#[meta(Tag::new())]
struct TaggedGateway;

#[messages]
impl TaggedGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[module(imports = [WsModule], providers = [TaggedGateway])]
struct TaggedModule;

/// Refused on the kernel's wiring step, whatever the app serves: no HTTP
/// transport runs here.
#[tokio::test]
async fn two_values_of_one_hidden_type_on_a_gateway_fail_the_boot_naming_it_and_the_gateway() {
    let Err(refused) = TestApp::builder()
        .module::<TaggedModule>()
        .build_headless()
        .await
    else {
        panic!("a gateway declaring a value read by nothing boots");
    };
    let wiring = refused
        .downcast_ref::<WiringFailedError>()
        .unwrap_or_else(|| panic!("the wiring step refuses it: {refused:#}"));
    assert_eq!(wiring.registry, "nest_rs::ws::gateways");
    let duplicate = refused
        .chain()
        .find_map(|cause| cause.downcast_ref::<DuplicateMetaError>())
        .unwrap_or_else(|| panic!("not a duplicate value: {refused:#}"));
    assert!(duplicate.type_name.ends_with("::Tag"), "{duplicate:?}");
    assert_eq!(duplicate.declared_on, "TaggedGateway");
}

/// Told once the connect hook started, which then never ends.
static CONNECT_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// The floor every per-unit timeout is held to: a second.
const HOOK_DEADLINE: Duration = Duration::from_secs(1);

#[gateway(path = "/stalled")]
struct StalledGateway;

#[messages]
impl StalledGateway {
    #[on_connect]
    async fn joined(&self) {
        CONNECT_STARTED.notify_one();
        std::future::pending::<()>().await;
    }

    #[subscribe_message("echo")]
    #[public]
    async fn echo(&self, text: String) -> String {
        text
    }
}

#[module(
    imports = [WsModule, WsModule::for_root(WsConfig {
        connect_timeout: Some(HOOK_DEADLINE),
        ..WsConfig::default()
    })],
    providers = [StalledGateway],
)]
struct StalledModule;

/// A connect hook that never ends would hold its socket before the read loop
/// and the lifetime ceiling are armed.
#[tokio::test]
async fn a_connect_hook_past_its_deadline_is_cut_and_its_socket_closed_unserved() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<StalledModule>()
        .build_ws()
        .await
        .expect("a pinned connect deadline boots");

    let mut socket = app.socket("/stalled").connect().await;
    CONNECT_STARTED.notified().await;
    // The deadline passes on paused time, while nothing reads.
    tokio::time::pause();
    tokio::time::sleep(HOOK_DEADLINE + Duration::from_millis(1)).await;
    tokio::time::resume();

    let (code, _) = socket.expect_close().await;
    assert_eq!(
        code,
        CloseCode::Error,
        "§7.4.1 1011, as a hook that panicked"
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::CONNECT.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
    );
    let cut = logs.expect_one(
        nest_rs_ws::TARGET,
        "websocket connection hook not finished within its deadline; it is cut",
    );
    assert_eq!(cut.level, "warn");
    assert_eq!(cut.field("unit").as_deref(), Some("ws.connect"));
    assert_eq!(cut.field("timeout_ms").as_deref(), Some("1000"));
    app.shutdown().await.expect("the transport stops cleanly");
}

/// Told once the disconnect hook started, which then never ends.
static DISCONNECT_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[gateway(path = "/lingering")]
struct LingeringGateway;

#[messages]
impl LingeringGateway {
    #[on_disconnect]
    async fn left(&self) {
        DISCONNECT_STARTED.notify_one();
        std::future::pending::<()>().await;
    }

    #[subscribe_message("echo")]
    #[public]
    async fn echo(&self, text: String) -> String {
        text
    }
}

#[module(
    imports = [WsModule, WsModule::for_root(WsConfig {
        disconnect_timeout: Some(HOOK_DEADLINE),
        ..WsConfig::default()
    })],
    providers = [LingeringGateway],
)]
struct LingeringModule;

/// A peer that left waits on no disconnect hook past its deadline: the hook
/// is cut, and the connection lets its registry entry go.
#[tokio::test]
async fn a_disconnect_hook_past_its_deadline_is_cut_and_the_close_goes_on() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<LingeringModule>()
        .build_ws()
        .await
        .expect("a pinned disconnect deadline boots");
    let server = app
        .container()
        .get::<nest_rs_ws::WsServer>()
        .expect("WsModule provides the registry");

    let socket = app.socket("/lingering").connect().await;
    drop(socket);
    DISCONNECT_STARTED.notified().await;
    // The deadline passes on paused time, while nothing reads.
    tokio::time::pause();
    tokio::time::sleep(HOOK_DEADLINE + Duration::from_millis(1)).await;
    tokio::time::resume();

    nest_rs_testing::wait_until(Duration::from_secs(5), || server.connection_count() == 0).await;
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::DISCONNECT.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
    );
    let cut = logs.expect_one(
        nest_rs_ws::TARGET,
        "websocket connection hook not finished within its deadline; it is cut",
    );
    assert_eq!(cut.field("unit").as_deref(), Some("ws.disconnect"));
    app.shutdown().await.expect("the transport stops cleanly");
}

/// The server unable to serve the message in time, said by the service.
#[derive(Debug, thiserror::Error)]
#[error("statement cancelled at its bound")]
struct Cancelled;

impl nest_rs_core::ToProblem for Cancelled {
    fn to_problem(&self) -> Option<Problem> {
        Some(Problem::new(503, code::UNAVAILABLE))
    }
}

#[gateway(path = "/slow")]
struct SlowGateway;

#[messages]
impl SlowGateway {
    #[subscribe_message("cancelled")]
    #[public]
    async fn cancelled(&self) -> Result<String, Cancelled> {
        Err(Cancelled)
    }
}

#[module(imports = [WsModule], providers = [SlowGateway])]
struct SlowModule;

/// Both a statement the database cancelled and the gateway's own deadline
/// answer `UNAVAILABLE`, the code a client branches on; `reason: "timeout"`
/// marks the deadline the gateway enforces, never a service's own problem.
#[tokio::test]
async fn a_service_unavailable_in_time_answers_its_code_and_no_deadline_reason() {
    let app = TestApp::builder()
        .module::<SlowModule>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let mut socket = app.socket("/slow").connect().await;
    socket.send("cancelled", serde_json::Value::Null).await;
    let frame = socket.next_envelope().await;
    assert_eq!(frame["data"]["code"], "UNAVAILABLE", "{frame}");
    assert!(frame["data"].get("reason").is_none(), "{frame}");
    assert_eq!(
        frame["data"]["error"],
        nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE,
        "{frame}"
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[gateway(path = "/refusing")]
struct RefusingGateway;

#[messages]
impl RefusingGateway {
    #[subscribe_message("full")]
    #[public]
    async fn full(&self) -> Result<String, nest_rs_ws::WsError> {
        Err(nest_rs_ws::WsError::new("no room left"))
    }

    #[subscribe_message("conflict")]
    #[public]
    async fn conflict(&self) -> nest_rs_core::anyhow::Result<String> {
        Err(
            nest_rs_core::anyhow::Error::from(Problem::new(409, code::CONFLICT))
                .context("sku-42 is reserved at the eu-west warehouse"),
        )
    }

    #[subscribe_message("piped")]
    #[public]
    async fn piped(&self) -> Result<String, nest_rs_pipes::PipeError> {
        Err(nest_rs_pipes::PipeError::new("not for sale"))
    }

    #[subscribe_message("undecodable")]
    #[public]
    async fn undecodable(&self) -> Result<u64, serde_json::Error> {
        serde_json::from_str(r#""sk_live_secret""#)
    }

    #[subscribe_message("unavailable")]
    #[public]
    async fn unavailable(&self) -> Result<String, Cancelled> {
        Err(Cancelled)
    }

    #[subscribe_message("failed")]
    #[public]
    async fn failed(&self) -> nest_rs_core::anyhow::Result<String> {
        Err(
            nest_rs_core::anyhow::Error::from(Problem::new(500, code::INTERNAL))
                .context("the ledger at the eu-west warehouse refused"),
        )
    }
}

#[module(imports = [WsModule], providers = [RefusingGateway])]
struct RefusingModule;

/// A refusal a handler answers is the client's error: its message's line
/// files it `error`, and the gateway files nothing beside it, as HTTP files a
/// `4xx` on its access line alone. The chain behind a `503` is filed at
/// `warn`, the server unable to serve it in time, and the chain a `5xx`
/// withholds at `error`.
#[tokio::test]
async fn a_refusal_a_handler_answers_files_its_line_alone() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<RefusingModule>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let mut socket = app.socket("/refusing").connect().await;
    let answered = [
        ("full", None),
        ("conflict", Some("CONFLICT")),
        ("piped", Some("INVALID_ARGUMENT")),
        ("undecodable", Some("INVALID_ARGUMENT")),
        ("unavailable", Some("UNAVAILABLE")),
        ("failed", Some("INTERNAL")),
    ];
    for (event, code) in answered {
        socket.send(event, serde_json::Value::Null).await;
        let frame = socket.next_envelope().await;
        assert_eq!(frame["event"], event, "{frame}");
        assert!(frame["data"]["error"].is_string(), "{frame}");
        assert_eq!(frame["data"]["code"], serde_json::json!(code), "{frame}");
        let said = frame.to_string();
        assert!(
            !said.contains("sk_live") && !said.contains("eu-west"),
            "{frame}"
        );
    }
    app.shutdown().await.expect("the transport stops cleanly");

    let lines = logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::MESSAGE.name(),
    );
    for (event, _) in answered {
        let filed: Vec<_> = lines
            .iter()
            .filter(|line| line.field("event").as_deref() == Some(event))
            .collect();
        let [line] = filed.as_slice() else {
            panic!("{event}: one line, {lines:#?}");
        };
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::ERROR),
            "{event}",
        );
    }
    let beside: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| event.target == nest_rs_ws::TARGET)
        .filter(|event| event.level == "warn" || event.level == "error")
        .collect();
    let said: Vec<_> = beside
        .iter()
        .map(|line| {
            (
                line.level.as_str(),
                line.message.as_str(),
                line.field("event").unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            (
                "warn",
                "websocket message unavailable",
                "unavailable".to_owned()
            ),
            ("error", "websocket message failed", "failed".to_owned()),
        ],
        "{beside:#?}",
    );
}

/// A frame no handler reads is a message all the same: a unit-stage interceptor
/// reads what an event no handler serves, a frame that is no envelope and a
/// binary frame are answered, under `_OTHER`, never the client's word.
#[tokio::test]
async fn the_unit_stage_reads_what_a_frame_no_handler_reads_is_answered() {
    let app = TestApp::builder()
        .module::<ReadModule>()
        .build_ws()
        .await
        .expect("a module bringing a unit-stage interceptor boots");

    let mut socket = app.socket("/read").connect().await;
    socket.send("marked:nope", serde_json::Value::Null).await;
    assert_eq!(
        socket.next_envelope().await["data"]["error"],
        "unknown event"
    );
    socket.send_text("no envelope").await;
    assert_eq!(socket.next_envelope().await["event"], "error");
    socket.send_binary(vec![1, 2, 3]).await;
    assert_eq!(socket.next_envelope().await["event"], "error");
    app.shutdown().await.expect("the transport stops cleanly");

    let read = READ_AT_THE_UNIT_STAGE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(read.len(), 3, "{read:?}");
    assert_eq!(read[0], "error: unknown event", "{read:?}");
    assert!(read[1].starts_with("error: invalid envelope"), "{read:?}");
    assert_eq!(
        read[2], "error: binary frames are not supported",
        "{read:?}"
    );
    let events = EVENTS_AT_THE_UNIT_STAGE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(
        events,
        [nest_rs_core::operation_log::OTHER; 3],
        "a layer reads the server's word for it, never the client's",
    );
}

/// Answers every frame an error carries, in place of it.
#[injectable]
#[derive(Default)]
struct Shapes;

impl Layer for Shapes {}

#[async_trait]
impl ExceptionFilter for Shapes {
    type Exception = nest_rs_ws::WsError;

    async fn catch(&self, refused: &nest_rs_ws::WsError, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(
            Problem::new(404, code::NOT_FOUND).with_detail(format!("shaped: {}", refused.error)),
        )
    }
}

#[module(
    imports = [WsModule],
    providers = [GlobalExceptionFilters<(Shapes,)>, Closed, ReadGateway],
)]
struct ShapedModule;

/// As an app's catch-all shapes a route's `404`, it shapes what a frame no
/// handler reads is answered.
#[tokio::test]
async fn an_apps_exception_filter_shapes_what_a_frame_no_handler_reads_is_answered() {
    let app = TestApp::builder()
        .module::<ShapedModule>()
        .build_ws()
        .await
        .expect("an app-wide exception filter boots");

    let mut socket = app.socket("/read").connect().await;
    socket.send("nope", serde_json::Value::Null).await;
    let frame = socket.next_envelope().await;
    assert_eq!(frame["event"], "nope", "{frame}");
    assert_eq!(frame["data"]["error"], "shaped: unknown event", "{frame}");
    socket.send_binary(vec![1, 2, 3]).await;
    let frame = socket.next_envelope().await;
    assert_eq!(frame["event"], "error", "{frame}");
    assert_eq!(
        frame["data"]["error"], "shaped: binary frames are not supported",
        "{frame}"
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

/// Admits every upgrade and refuses every message it checks.
#[injectable]
#[derive(Default)]
struct ClosedToMessages;

impl Layer for ClosedToMessages {}

#[async_trait]
impl Guard for ClosedToMessages {
    async fn check_http(&self, _: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    async fn check(&self, _: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("closed for the night"))
    }
}

#[module(
    imports = [WsModule],
    providers = [GlobalGuards<(ClosedToMessages,)>, Closed, ReadGateway],
)]
struct PooledGuardModule;

/// What no handler reads has no decision to take: the pool's guards refuse the
/// messages a handler serves and run at no other, as they run at no route's
/// `404`.
#[tokio::test]
async fn a_frame_no_handler_reads_runs_no_guard() {
    let app = TestApp::builder()
        .module::<PooledGuardModule>()
        .build_ws()
        .await
        .expect("a pooled guard boots");

    let mut socket = app.socket("/read").connect().await;
    socket.send("open", serde_json::Value::Null).await;
    assert_eq!(
        socket.next_envelope().await["data"]["error"],
        "closed for the night"
    );
    socket.send("nope", serde_json::Value::Null).await;
    assert_eq!(
        socket.next_envelope().await["data"]["error"],
        "unknown event"
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

/// Told once [`Stalling`] started waiting.
static STALLING: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Waits forever around what no handler reads.
#[nest_rs_core::interceptor(stage = unit)]
#[derive(Default)]
struct Stalling;

impl Layer for Stalling {}

#[async_trait]
impl Interceptor for Stalling {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        if cx
            .switch_to::<WsView>()
            .is_some_and(|view| view.event() == nest_rs_core::operation_log::OTHER)
        {
            STALLING.notify_one();
            std::future::pending::<()>().await;
        }
        next.run(cx).await
    }
}

#[module(
    imports = [WsModule, WsModule::for_root(WsConfig {
        message_timeout: Some(Duration::from_secs(1)),
        ..WsConfig::default()
    })],
    providers = [Stalling, Closed, ReadGateway],
)]
struct StallingModule;

/// A layer waits on the framework's own bound there too: the message's
/// deadline cuts it, and the socket goes on.
#[tokio::test]
async fn a_layer_stalling_on_a_frame_no_handler_reads_is_cut_at_the_message_deadline() {
    let app = TestApp::builder()
        .module::<StallingModule>()
        .build_ws()
        .await
        .expect("a unit-stage interceptor boots");

    let mut socket = app.socket("/read").connect().await;
    socket.send_binary(vec![1, 2, 3]).await;
    tokio::time::timeout(Duration::from_secs(5), STALLING.notified())
        .await
        .expect("the layer runs around the frame");
    tokio::time::pause();
    tokio::time::sleep(Duration::from_millis(1001)).await;
    tokio::time::resume();

    let frame = socket.next_envelope().await;
    assert_eq!(frame["event"], "error", "{frame}");
    assert_eq!(frame["data"]["reason"], "timeout", "{frame}");
    socket.send("open", serde_json::Value::Null).await;
    assert_eq!(socket.next_envelope().await["data"], "welcome");
    app.shutdown().await.expect("the transport stops cleanly");
}

/// Unwinds around what no handler reads.
#[nest_rs_core::interceptor(stage = unit)]
#[derive(Default)]
struct Exploding;

impl Layer for Exploding {}

#[async_trait]
impl Interceptor for Exploding {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        assert!(
            cx.switch_to::<WsView>()
                .is_none_or(|view| view.event() != nest_rs_core::operation_log::OTHER),
            "the layer exploded",
        );
        next.run(cx).await
    }
}

#[module(imports = [WsModule], providers = [Exploding, Closed, ReadGateway])]
struct ExplodingModule;

/// A panic is contained where a message's is: the frame's line files `panic`,
/// the client reads the opaque frame, and the socket goes on.
#[tokio::test]
async fn a_layer_unwinding_on_a_frame_no_handler_reads_keeps_the_socket() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<ExplodingModule>()
        .build_ws()
        .await
        .expect("a unit-stage interceptor boots");

    let mut socket = app.socket("/read").connect().await;
    socket.send_binary(vec![1, 2, 3]).await;
    let frame = socket.next_envelope().await;
    assert_eq!(frame["data"]["code"], code::INTERNAL.as_str(), "{frame}");
    socket.send("open", serde_json::Value::Null).await;
    assert_eq!(socket.next_envelope().await["data"], "welcome");
    app.shutdown().await.expect("the transport stops cleanly");

    let lines = logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::MESSAGE.name(),
    );
    assert_eq!(
        lines[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
        "{lines:#?}",
    );
}

/// Answers what no handler reads itself: a reply where the frame carried
/// `data`, silence where it did not.
#[nest_rs_core::interceptor(stage = unit)]
#[derive(Default)]
struct AnswersInstead;

impl Layer for AnswersInstead {}

#[async_trait]
impl Interceptor for AnswersInstead {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        if let Some(view) = cx.switch_to_mut::<WsView>()
            && view.event() == nest_rs_core::operation_log::OTHER
        {
            let reply = match view.data() {
                Some(serde_json::Value::Null) => WsReply::None,
                _ => WsReply::Reply(serde_json::json!("answered")),
            };
            return Ok(view.respond(reply));
        }
        next.run(cx).await
    }
}

#[module(imports = [WsModule], providers = [AnswersInstead, Closed, ReadGateway])]
struct AnswersInsteadModule;

/// A layer answers such a frame as it answers a message: on the event the frame
/// named, or with silence, the next frame the client reads being the next
/// message's. The unit did not fail, so its line files `ok`.
#[tokio::test]
async fn a_layer_answers_a_frame_no_handler_reads_in_place_of_the_edge() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<AnswersInsteadModule>()
        .build_ws()
        .await
        .expect("a unit-stage interceptor boots");

    let mut socket = app.socket("/read").connect().await;
    socket.send("nope", serde_json::json!("hi")).await;
    let frame = socket.next_envelope().await;
    assert_eq!(frame["event"], "nope", "{frame}");
    assert_eq!(frame["data"], "answered", "{frame}");
    socket.send_binary(vec![1, 2, 3]).await;
    socket.send("open", serde_json::Value::Null).await;
    assert_eq!(
        socket.next_envelope().await["data"],
        "welcome",
        "the binary frame was answered with silence",
    );
    app.shutdown().await.expect("the transport stops cleanly");

    let lines = logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::MESSAGE.name(),
    );
    assert_eq!(lines.len(), 3, "{lines:#?}");
    for line in &lines {
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::OK),
            "{line:#?}",
        );
    }
}

/// A failure of the server's own, said by a service's domain error.
#[derive(Debug, thiserror::Error)]
#[error("the replica is gone")]
struct ReplicaGone;

impl nest_rs_core::ToProblem for ReplicaGone {
    fn to_problem(&self) -> Option<Problem> {
        None
    }
}

/// A failure its client earned, said by a service's domain error, whose chain
/// names no vocabulary: only the edge rendering it knows the class.
#[derive(Debug, thiserror::Error)]
#[error("seat 12 is taken")]
struct SeatTaken;

impl nest_rs_core::ToProblem for SeatTaken {
    fn to_problem(&self) -> Option<Problem> {
        Some(Problem::new(409, code::CONFLICT))
    }
}

/// A failure its client earned, in an error that cannot cross threads: the
/// exception filters read a sentence of it, never its type.
#[derive(Debug)]
struct RoomFull {
    cause: std::rc::Rc<Problem>,
}

impl std::fmt::Display for RoomFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the room is full")
    }
}

impl std::error::Error for RoomFull {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.cause)
    }
}

/// Fails the message before its handler with a failure of the server's own, the
/// edge having rendered nothing yet.
#[injectable]
#[derive(Default)]
struct RaisesDisk;

impl Layer for RaisesDisk {}

#[async_trait]
impl Interceptor for RaisesDisk {
    async fn intercept(&self, _: &mut UnitContext<'_>, _: Next<'_>) -> Outcome {
        Err(std::io::Error::other("the disk is gone").into())
    }
}

/// Fails the message before its handler with a failure its client earned, the
/// edge having rendered nothing yet.
#[injectable]
#[derive(Default)]
struct RaisesConflict;

impl Layer for RaisesConflict {}

#[async_trait]
impl Interceptor for RaisesConflict {
    async fn intercept(&self, _: &mut UnitContext<'_>, _: Next<'_>) -> Outcome {
        Err(Problem::new(409, code::CONFLICT).into())
    }
}

#[gateway(path = "/who-failed")]
struct WhoFailedGateway;

#[messages]
impl WhoFailedGateway {
    #[subscribe_message("disk")]
    #[public]
    async fn disk(&self) -> Result<String, std::io::Error> {
        Err(std::io::Error::other("the disk is gone"))
    }

    #[subscribe_message("failed")]
    #[public]
    async fn failed(&self) -> Result<String, Problem> {
        Err(Problem::new(500, code::INTERNAL))
    }

    #[subscribe_message("replica")]
    #[public]
    async fn replica(&self) -> Result<String, ReplicaGone> {
        Err(ReplicaGone)
    }

    #[subscribe_message("raised_disk")]
    #[use_interceptors(RaisesDisk)]
    #[public]
    async fn raised_disk(&self) -> String {
        "never".to_owned()
    }

    #[subscribe_message("conflict")]
    #[public]
    async fn conflict(&self) -> Result<String, Problem> {
        Err(Problem::new(409, code::CONFLICT))
    }

    #[subscribe_message("seat")]
    #[public]
    async fn seat(&self) -> Result<String, SeatTaken> {
        Err(SeatTaken)
    }

    #[subscribe_message("room")]
    #[public]
    async fn room(&self) -> Result<String, RoomFull> {
        Err(RoomFull {
            cause: std::rc::Rc::new(Problem::new(409, code::CONFLICT)),
        })
    }

    #[subscribe_message("deliberate")]
    #[public]
    async fn deliberate(&self) -> Result<String, nest_rs_ws::WsError> {
        Err(nest_rs_ws::WsError::new("not today"))
    }

    #[subscribe_message("typed")]
    #[public]
    async fn typed(&self, _count: u32) -> String {
        "typed".to_owned()
    }

    #[subscribe_message("raised_conflict")]
    #[use_interceptors(RaisesConflict)]
    #[public]
    async fn raised_conflict(&self) -> String {
        "never".to_owned()
    }
}

/// The failures of the server's own [`WhoFailedGateway`] answers.
const THE_SERVERS: [&str; 4] = ["disk", "failed", "replica", "raised_disk"];

/// The failures its client earned, what no handler reads last.
const THE_CLIENTS: [&str; 7] = [
    "conflict",
    "seat",
    "room",
    "deliberate",
    "typed",
    "raised_conflict",
    "nope",
];

/// Sends `socket` every failure of [`WhoFailedGateway`], reading each answer with
/// `read`, then says who failed on each span: an error for the server's own,
/// nothing for what its client earned, every line filing `error`.
async fn assert_who_failed(
    logs: &LogCapture,
    app: nest_rs_testing::ws::WsApp,
    read: impl Fn(&serde_json::Value),
) {
    let mut socket = app.socket("/who-failed").connect().await;
    for event in THE_SERVERS.into_iter().chain(THE_CLIENTS) {
        let data = if event == "typed" {
            serde_json::json!("no number")
        } else {
            serde_json::Value::Null
        };
        socket.send(event, data).await;
        read(&socket.next_envelope().await);
    }
    app.shutdown().await.expect("the transport stops cleanly");

    let lines = logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::MESSAGE.name(),
    );
    assert_eq!(
        lines.len(),
        THE_SERVERS.len() + THE_CLIENTS.len(),
        "{lines:#?}"
    );
    for line in &lines {
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::ERROR),
            "an answer leaves the unit failed: {line:#?}",
        );
    }
    for event in THE_SERVERS {
        assert_span_records(logs, event, Some(nest_rs_core::operation_log::ERROR));
    }
    for event in THE_CLIENTS.map(|event| match event {
        "nope" => nest_rs_core::operation_log::OTHER,
        event => event,
    }) {
        assert_span_records(logs, event, None);
    }
}

/// `event`'s span records `error_type` and, with one, the `Error` status.
fn assert_span_records(logs: &LogCapture, event: &str, error_type: Option<&str>) {
    let span = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_ws::unit::MESSAGE.name()
                && span.field("ws.event").as_deref() == Some(event)
        })
        .unwrap_or_else(|| panic!("the span of `{event}`"));
    assert_eq!(
        span.field("error.type").as_deref(),
        error_type,
        "{event}: {:?}",
        span.fields
    );
    assert_eq!(
        span.field("otel.status_code").as_deref(),
        error_type.map(|_| "error"),
        "{event}: {:?}",
        span.fields,
    );
}

/// A catch-all writing its house frame in place of every failure's.
#[injectable]
#[derive(Default)]
struct HouseFrame;

impl Layer for HouseFrame {}

#[async_trait]
impl ExceptionFilter for HouseFrame {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        match cx.switch_to_mut::<WsView>() {
            Some(view) => Caught::Answered(view.respond(WsReply::error("something went wrong"))),
            None => Caught::Declined,
        }
    }
}

#[module(
    imports = [WsModule],
    providers = [
        GlobalExceptionFilters<(HouseFrame,)>,
        RaisesDisk,
        RaisesConflict,
        WhoFailedGateway,
    ],
)]
struct HouseFrameModule;

/// A filter's frame changes what the client reads, never who failed: the frame
/// names no status, and the server's own failures still record behind it.
#[tokio::test]
async fn a_filters_own_frame_leaves_who_failed_on_the_span() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<HouseFrameModule>()
        .build_ws()
        .await
        .expect("an app-wide exception filter boots");
    assert_who_failed(&logs, app, |frame| {
        assert_eq!(frame["data"]["error"], "something went wrong", "{frame}");
    })
    .await;
}

/// A catch-all acknowledging every failure with a plain reply.
#[injectable]
#[derive(Default)]
struct PlainReply;

impl Layer for PlainReply {}

#[async_trait]
impl ExceptionFilter for PlainReply {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        match cx.switch_to_mut::<WsView>() {
            Some(view) => Caught::Answered(
                view.respond(WsReply::Reply(serde_json::json!({ "failed": true }))),
            ),
            None => Caught::Declined,
        }
    }
}

#[module(
    imports = [WsModule],
    providers = [
        GlobalExceptionFilters<(PlainReply,)>,
        RaisesDisk,
        RaisesConflict,
        WhoFailedGateway,
    ],
)]
struct PlainReplyModule;

/// A reply is no error frame, and the failure behind it still says who failed:
/// its client's sets nothing on the server's span, the server's own still does.
#[tokio::test]
async fn a_filters_plain_reply_leaves_who_failed_on_the_span() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<PlainReplyModule>()
        .build_ws()
        .await
        .expect("an app-wide exception filter boots");
    assert_who_failed(&logs, app, |frame| {
        assert_eq!(frame["data"]["failed"], true, "{frame}");
    })
    .await;
}

/// Answers whatever it catches as the client's conflict.
#[injectable]
#[derive(Default)]
struct AsConflict;

impl Layer for AsConflict {}

#[async_trait]
impl ExceptionFilter for AsConflict {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(409, code::CONFLICT))
    }
}

/// Answers whatever it catches as the server's failure.
#[injectable]
#[derive(Default)]
struct AsFailure;

impl Layer for AsFailure {}

#[async_trait]
impl ExceptionFilter for AsFailure {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(500, code::INTERNAL))
    }
}

#[gateway(path = "/reclassed")]
struct ReclassedGateway;

#[messages]
impl ReclassedGateway {
    #[subscribe_message("disk_as_conflict")]
    #[use_exception_filters(AsConflict)]
    #[public]
    async fn disk_as_conflict(&self) -> Result<String, std::io::Error> {
        Err(std::io::Error::other("the disk is gone"))
    }

    #[subscribe_message("conflict_as_failure")]
    #[use_exception_filters(AsFailure)]
    #[public]
    async fn conflict_as_failure(&self) -> Result<String, Problem> {
        Err(Problem::new(409, code::CONFLICT))
    }
}

#[module(imports = [WsModule], providers = [AsConflict, AsFailure, ReclassedGateway])]
struct ReclassedModule;

/// A problem a filter answers names a status, which decides, as HTTP's filter's
/// does: what it answers `4xx` the server does not record, what it answers `5xx`
/// it does.
#[tokio::test]
async fn a_filters_problem_decides_who_failed_by_its_status() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<ReclassedModule>()
        .build_ws()
        .await
        .expect("a message's exception filters boot");
    let mut socket = app.socket("/reclassed").connect().await;
    socket
        .send("disk_as_conflict", serde_json::Value::Null)
        .await;
    assert_eq!(socket.next_envelope().await["data"]["code"], "CONFLICT");
    socket
        .send("conflict_as_failure", serde_json::Value::Null)
        .await;
    assert_eq!(socket.next_envelope().await["data"]["code"], "INTERNAL");
    app.shutdown().await.expect("the transport stops cleanly");

    assert_span_records(&logs, "disk_as_conflict", None);
    assert_span_records(
        &logs,
        "conflict_as_failure",
        Some(nest_rs_core::operation_log::ERROR),
    );
}
