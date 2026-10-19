//! A message on the pipeline, over a real upgrade: the gateway's layers and its
//! own, the app's, its request scope, what its upgrade attached, the boot
//! checks its gateway's wiring runs, and its connection hooks' deadlines.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use nest_rs_core::problem::code;
use nest_rs_core::{
    Caught, DuplicateMetaError, ExceptionFilter, Interceptor, Layer, Next, Outcome, Problem,
    UnitContext, WiringFailedError, injectable, module,
};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_testing::{LogCapture, TestApp};
use nest_rs_ws::{
    CloseCode, Scoped, WsConfig, WsModule, WsReply, WsView, async_trait, gateway, messages,
};

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

#[tokio::test]
async fn the_apps_interceptors_run_around_every_message() {
    let app = TestApp::builder()
        .module::<ListedModule>()
        .use_interceptors_global([nest_rs_core::interceptor::<Listed>()])
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
    assert_eq!(frame["data"]["errors"]["code"], "CONFLICT", "{frame}");
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
                None => "nothing".to_owned(),
            };
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
}

impl GuardFor<nest_rs_ws::nest_rs_http::HttpView> for AcmeUpgrade {}

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
    assert_eq!(cut.field("timeout_secs").as_deref(), Some("1"));
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
    assert_eq!(frame["data"]["errors"]["code"], "UNAVAILABLE", "{frame}");
    assert!(frame["data"]["errors"].get("reason").is_none(), "{frame}");
    assert_eq!(
        frame["data"]["error"],
        nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE,
        "{frame}"
    );
    app.shutdown().await.expect("the transport stops cleanly");
}
