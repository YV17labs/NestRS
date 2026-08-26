//! `#[messages]`-generated `Gateway::dispatch` — the return-type shape paths
//! the macro picks (Unit / Value / `Result<(), E>` / `Result<T, E>`) — and the
//! address the generated `Discoverable` mounts that dispatcher at. The macro
//! itself lives in `nest-rs-ws-macros`; this file pins its observable behaviour.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_pipes::{Pipe, PipeError, Piped, Trim, Valid};
use nest_rs_testing::TestApp;
use nest_rs_ws::nest_rs_http::poem::Request as HttpRequest;
use nest_rs_ws::{Gateway, WsClient, WsModule, WsReply, async_trait, gateway, messages};
use poem::http::{StatusCode, header};
use serde::{Deserialize, Serialize};
use validator::Validate;

/// A typed error that is **`Serialize`** and whose `Display` deliberately
/// withholds a field — the shape that made the alias leak matter.
#[derive(Debug, Serialize)]
struct DbFailure {
    dsn: String,
    message: String,
}

impl std::fmt::Display for DbFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DbFailure {}

/// The alias the return-type detection cannot see through.
type ServiceResult<T> = Result<T, DbFailure>;

/// A pipe that always rejects — exercises the WS pipe error path.
struct Reject;

impl Pipe for Reject {
    type In = String;
    type Out = String;
    fn transform(_: String) -> Result<String, PipeError> {
        Err(PipeError::new("bad input"))
    }
}

#[derive(Deserialize, Validate)]
struct NameInput {
    #[validate(length(min = 1))]
    name: String,
}

#[gateway(path = "/test")]
pub struct TestGateway;

#[messages]
impl TestGateway {
    #[subscribe_message("ok")]
    #[public]
    async fn ok_handler(&self) -> Result<String, std::io::Error> {
        Ok("yay".to_string())
    }

    #[subscribe_message("err")]
    #[public]
    async fn err_handler(&self) -> Result<String, std::io::Error> {
        Err(std::io::Error::other("boom"))
    }

    #[subscribe_message("ok_unit")]
    #[public]
    async fn ok_unit_handler(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[subscribe_message("err_unit")]
    #[public]
    async fn err_unit_handler(&self) -> Result<(), std::io::Error> {
        Err(std::io::Error::other("boom-unit"))
    }

    #[subscribe_message("plain")]
    #[public]
    async fn plain_handler(&self) -> String {
        "hello".to_string()
    }

    #[subscribe_message("nothing")]
    #[public]
    async fn nothing_handler(&self) {}

    // `Piped<Trim, String>`: the wire payload is a `String`; the handler sees it
    // trimmed — the WS analog of the HTTP / GraphQL / queue pipe forms.
    #[subscribe_message("trim")]
    #[public]
    async fn trim_handler(&self, name: Piped<Trim, String>) -> String {
        name.into_inner()
    }

    // A rejecting pipe replies with an error frame, never reaching the body.
    #[subscribe_message("checked")]
    #[public]
    async fn checked_handler(&self, name: Piped<Reject, String>) -> String {
        name.into_inner()
    }

    // `Valid<T>`: validates the deserialized payload before the handler runs.
    #[subscribe_message("named")]
    #[public]
    async fn named_handler(&self, input: Valid<NameInput>) -> String {
        input.into_inner().name
    }

    // The two spellings of one type. `literal` is what the macro can see; the
    // three `renamed_*` handlers are the same `Result` behind an alias.
    #[subscribe_message("literal")]
    #[public]
    async fn literal_handler(&self) -> Result<String, DbFailure> {
        Err(failure())
    }

    #[subscribe_message("renamed")]
    #[public]
    async fn renamed_handler(&self) -> ServiceResult<String> {
        Err(failure())
    }

    #[subscribe_message("renamed_ok")]
    #[public]
    async fn renamed_ok_handler(&self) -> ServiceResult<String> {
        Ok("fine".to_string())
    }
}

fn failure() -> DbFailure {
    DbFailure {
        dsn: "postgres://blog:hunter2@db:5432/blog".to_string(),
        message: "database unavailable".to_string(),
    }
}

#[tokio::test]
async fn result_ok_serializes_to_reply() {
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "ok", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Reply(v) => assert_eq!(v.as_str(), Some("yay")),
        _ => panic!("expected Reply for Result::Ok(T)"),
    }
}

#[tokio::test]
async fn result_err_becomes_error_frame() {
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "err", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Error(msg) => {
            assert!(msg.error.contains("boom"), "want 'boom' in {msg}");
        }
        _ => panic!("expected Error for Result::Err"),
    }
}

#[tokio::test]
async fn result_ok_unit_sends_none() {
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "ok_unit", serde_json::Value::Null)
        .await;
    assert!(
        matches!(reply, WsReply::None),
        "Result<(), E>::Ok(()) must send no reply",
    );
}

#[tokio::test]
async fn result_err_unit_becomes_error_frame() {
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "err_unit", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Error(msg) => {
            assert!(msg.error.contains("boom-unit"), "want 'boom-unit' in {msg}");
        }
        _ => panic!("expected Error for Result<(), E>::Err"),
    }
}

#[tokio::test]
async fn plain_value_serializes_to_reply() {
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "plain", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Reply(v) => assert_eq!(v.as_str(), Some("hello")),
        _ => panic!("expected Reply for a plain T return"),
    }
}

#[tokio::test]
async fn unit_return_sends_none() {
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "nothing", serde_json::Value::Null)
        .await;
    assert!(
        matches!(reply, WsReply::None),
        "() return must send no reply",
    );
}

#[tokio::test]
async fn unknown_event_returns_unknown_error() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "missing", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Error(msg) => {
            assert!(
                msg.error.contains("missing") && msg.error.contains("unknown"),
                "want 'unknown' + the event name in {msg}",
            );
        }
        _ => panic!("expected Error for an unrouted event"),
    }

    // A socket stays open after an unrouted event, so nothing surfaces at the
    // transport: a client typo and a deploy that dropped a `#[subscribe_message]`
    // look identical from the outside and both keep the connection alive. The
    // event is what separates them, and it is the only per-message trace on a
    // long-lived connection.
    let event = logs.expect_one(
        "nest_rs::ws",
        "subscribe_message dispatched to an unknown event",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("event").as_deref(), Some("missing"));
}

#[tokio::test]
async fn a_piped_payload_runs_the_pipe_before_the_handler() {
    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "trim",
            serde_json::Value::String("  hi  ".to_string()),
        )
        .await;
    match reply {
        WsReply::Reply(v) => assert_eq!(v.as_str(), Some("hi")),
        _ => panic!("expected the trimmed payload"),
    }
}

#[tokio::test]
async fn a_rejecting_pipe_replies_with_an_error_frame() {
    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "checked",
            serde_json::Value::String("whatever".to_string()),
        )
        .await;
    match reply {
        WsReply::Error(msg) => {
            assert!(msg.error.contains("bad input"), "want 'bad input' in {msg}")
        }
        _ => panic!("expected an error frame from the rejecting pipe"),
    }
}

#[tokio::test]
async fn a_valid_payload_is_validated_before_the_handler() {
    let ok = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "named",
            serde_json::json!({ "name": "ok" }),
        )
        .await;
    match ok {
        WsReply::Reply(v) => assert_eq!(v.as_str(), Some("ok")),
        _ => panic!("expected the validated name"),
    }

    let bad = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "named",
            serde_json::json!({ "name": "" }),
        )
        .await;
    match bad {
        WsReply::Error(msg) => {
            assert!(
                msg.error.contains("validation failed"),
                "want validation error in {msg}"
            );
            // The finding: the macro formatted only `PipeError::message()`, so a
            // client learned that validation failed and never which field. The
            // per-field detail rides the frame as `errors`, the member name HTTP
            // uses for the same rejection.
            let errors = msg
                .errors
                .as_ref()
                .unwrap_or_else(|| panic!("the frame must carry the field errors: {msg:?}"));
            assert!(
                errors.get("name").is_some(),
                "the offending field is named: {errors}",
            );
        }
        _ => panic!("expected a validation error frame"),
    }
}

/// The wire shape, not just the reply value: a client parses
/// `{ event, data: { error, errors } }`, and `errors` is absent — not `null` —
/// when the failure had no structured detail. Same asymmetry HTTP has.
#[tokio::test]
async fn the_error_frame_carries_error_and_errors_under_data() {
    let bad = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "named",
            serde_json::json!({ "name": "" }),
        )
        .await;
    let WsReply::Error(err) = bad else {
        panic!("expected a validation error frame");
    };
    let frame: serde_json::Value =
        serde_json::from_str(&nest_rs_ws::WsEnvelope::encode("named", &err).expect("encode"))
            .expect("parse");
    assert_eq!(frame["event"], "named");
    assert!(frame["data"]["error"].is_string(), "{frame}");
    assert!(frame["data"]["errors"]["name"].is_array(), "{frame}");

    let plain = nest_rs_ws::WsError::new("unknown event `nope`");
    let frame: serde_json::Value =
        serde_json::from_str(&nest_rs_ws::WsEnvelope::encode("nope", &plain).expect("encode"))
            .expect("parse");
    assert!(
        frame["data"].get("errors").is_none(),
        "no detail ⇒ no `errors` member at all: {frame}",
    );
}

/// A `Result` reached through a type alias must behave exactly like the literal
/// form. It did not: return-type detection is syntactic on the last path
/// segment, so `ServiceResult<T>` read as an ordinary value and the `Err`
/// variant was serialized into the reply `data` — the whole error struct,
/// including the field `Display` withholds, in a frame with no `error` key. It
/// compiled without a warning and logged nothing, because nothing knew a
/// failure had happened.
#[tokio::test]
async fn an_aliased_result_produces_an_error_frame_not_a_serialized_err() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "renamed", serde_json::Value::Null)
        .await;

    match reply {
        WsReply::Error(msg) => assert_eq!(msg.error, "database unavailable"),
        WsReply::Reply(value) => panic!("the Err variant was shipped as a success frame — {value}"),
        WsReply::None => panic!("expected an error frame"),
    }

    // …and the denial is greppable, on the transport's own target.
    let event = logs.expect_one("nest_rs::ws", "subscribe_message handler returned Err");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("event").as_deref(), Some("renamed"));
}

/// Both spellings produce the same frame — the point of the fix is that the
/// contract no longer depends on how the type was written.
#[tokio::test]
async fn the_literal_and_aliased_forms_reply_identically() {
    let mut frames = Vec::new();
    for event in ["literal", "renamed"] {
        match TestGateway
            .dispatch(&WsClient::for_test(), event, serde_json::Value::Null)
            .await
        {
            WsReply::Error(msg) => frames.push(msg.error),
            other => panic!(
                "`{event}` must produce an error frame, got {}",
                match other {
                    WsReply::Reply(v) => format!("a reply: {v}"),
                    _ => "silence".to_string(),
                }
            ),
        }
    }
    assert_eq!(frames[0], frames[1]);
    // The withheld field never reaches the wire on either path.
    assert!(!frames[0].contains("hunter2"), "{}", frames[0]);
}

/// The `Ok` half still replies with the value, so the fix costs the happy path
/// nothing — an aliased `Result` is now simply a `Result`.
#[tokio::test]
async fn an_aliased_result_still_replies_on_ok() {
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "renamed_ok", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Reply(v) => assert_eq!(v.as_str(), Some("fine")),
        _ => panic!("expected the Ok value"),
    }
}

// --- E4: a refused dispatch must be greppable, whoever refused it ---

/// `/websockets/messages/` promises "a `warn!` lands in the `nest_rs::ws`
/// target alongside the frame, so a denied dispatch shows up in logs without
/// extra instrumentation" — in the paragraph about a `Valid<T>` rejection.
///
/// Only a handler-returned `Err` warned. A pipe rejection and a malformed
/// payload — a *client* sending garbage, which is the case worth alerting on —
/// produced the right frame and no record at any level.
#[tokio::test]
async fn a_pipe_rejection_warns_on_the_ws_target() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "checked",
            serde_json::json!("anything"),
        )
        .await;
    assert!(matches!(reply, WsReply::Error(_)), "the frame is unchanged");

    let event = logs.expect_one("nest_rs::ws", "subscribe_message rejected by a pipe");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("event").as_deref(), Some("checked"));
}

#[tokio::test]
async fn a_structured_validation_rejection_warns_too() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "named",
            serde_json::json!({ "name": "" }),
        )
        .await;
    assert!(matches!(reply, WsReply::Error(_)));

    let event = logs.expect_one("nest_rs::ws", "subscribe_message rejected by a pipe");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("event").as_deref(), Some("named"));
}

#[tokio::test]
async fn a_malformed_payload_warns_on_the_ws_target() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "named",
            serde_json::json!({ "nope": 1 }),
        )
        .await;
    match reply {
        WsReply::Error(msg) => assert!(
            msg.error.contains("invalid payload for `named`"),
            "the frame is unchanged: {}",
            msg.error,
        ),
        WsReply::Reply(value) => panic!("expected an error frame, got a reply: {value}"),
        WsReply::None => panic!("expected an error frame, got silence"),
    }

    let event = logs.expect_one(
        "nest_rs::ws",
        "subscribe_message payload failed to deserialize",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("event").as_deref(), Some("named"));
}

// ── The mount address: `#[gateway(version = …)]` ────────────────────────────
//
// A gateway's mount is a path a client selects, so it declares a version the
// way a controller does and resolves it through the same
// `nest_rs_http::version_path`. What follows pins the three things that
// declaration decides: the effective path, what the router serves, and what the
// boot does with two gateways that share a path.

#[gateway(path = "/ws", version = "1")]
pub struct VersionedGateway;

#[messages]
impl VersionedGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        "v1".into()
    }
}

/// `version_path` prefixes, so a declared version moves the whole mount under
/// `/v{n}` — the same address shape `#[controller(version = "1")]` produces, and
/// the reason the argument is spelled identically on both edges.
#[test]
fn a_declared_version_moves_the_mount_under_its_segment() {
    assert_eq!(VersionedGateway::VERSION, Some("1"));
    assert_eq!(
        VersionedGateway::PATH,
        "/ws",
        "the declaration is untouched"
    );
    assert_eq!(VersionedGateway::__nestrs_mount_path(), "/v1/ws");
}

/// The other half, and the one that guarantees this is additive: a gateway that
/// declares no version mounts exactly where it always did.
#[test]
fn an_undeclared_version_leaves_the_mount_alone() {
    assert_eq!(TestGateway::VERSION, None);
    assert_eq!(TestGateway::__nestrs_mount_path(), "/test");
}

#[module(imports = [WsModule], providers = [VersionedGateway])]
struct VersionedModule;

/// What a socket-less client can prove about a mount, and it is more than it
/// looks: `400 invalid protocol` is poem's `WebSocket` extractor refusing a GET
/// that is not an upgrade, so only a mounted gateway endpoint can answer it.
/// `500 no upgrade` is that same extractor accepting the *whole* handshake —
/// method, `Upgrade`, `Connection`, `Sec-WebSocket-Version`, `Sec-WebSocket-Key`
/// — and finding no hyper upgrade seam, which is exactly as far as an in-process
/// `TestClient` reaches. Past that seam is `nest_rs_testing::ws`, which binds a
/// real port; the socket-driven witnesses are at the end of this file.
async fn upgrade_status(app: &TestApp, path: &str) -> StatusCode {
    app.http()
        .get(path)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "upgrade")
        .header(header::SEC_WEBSOCKET_VERSION, "13")
        .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .0
        .status()
}

#[tokio::test]
async fn a_versioned_gateway_serves_at_its_version_segment_and_nowhere_else() {
    let app = TestApp::for_module::<VersionedModule>()
        .await
        .expect("a versioned gateway boots like any other");

    assert_eq!(
        upgrade_status(&app, "/v1/ws").await,
        StatusCode::INTERNAL_SERVER_ERROR,
        "the handshake reached the gateway and passed every header check",
    );
    app.http()
        .get("/v1/ws")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    // …and the undeclared address is not a second door. A gateway that declared
    // a version and stayed reachable unversioned would be the silent-fallback
    // failure URI versioning exists to avoid.
    assert_eq!(upgrade_status(&app, "/ws").await, StatusCode::NOT_FOUND);
    app.http()
        .get("/ws")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

/// A boot log naming an address nobody can connect to is worse than none — it is
/// the first thing read when a socket will not open. Both events the mount emits
/// carry the effective path: the transport's, and the per-event one `#[messages]`
/// writes from inside the mount closure.
#[tokio::test]
async fn the_boot_log_names_the_address_a_client_connects_to() {
    let logs = nest_rs_testing::LogCapture::install();
    let _app = TestApp::for_module::<VersionedModule>()
        .await
        .expect("a versioned gateway boots");

    let endpoint = logs.expect_one("nest_rs::routes", "mounted endpoint");
    assert_eq!(endpoint.field("path").as_deref(), Some("/v1/ws"));
    assert_eq!(endpoint.field("kind").as_deref(), Some("ws"));

    let message = logs.expect_one("nest_rs::routes", "mounted message");
    assert_eq!(message.field("path").as_deref(), Some("/v1/ws"));
    assert_eq!(message.field("event").as_deref(), Some("ping"));
}

// A gateway owns its mount, so "which gateway is at this address" is a question
// with a testable answer: give each version a connection guard that denies with
// its own reason. The reason rides the problem+json `detail`, so the response
// names the gateway whose upgrade chain actually ran.

#[injectable]
#[derive(Default)]
struct V1Guard;

impl Layer for V1Guard {}

#[async_trait]
impl Guard for V1Guard {
    async fn check_http(&self, _req: &mut HttpRequest) -> Result<(), Denial> {
        Err(Denial::unauthorized("the v1 socket refused you"))
    }
}

impl HttpGuard for V1Guard {}

#[injectable]
#[derive(Default)]
struct V2Guard;

impl Layer for V2Guard {}

#[async_trait]
impl Guard for V2Guard {
    async fn check_http(&self, _req: &mut HttpRequest) -> Result<(), Denial> {
        Err(Denial::unauthorized("the v2 socket refused you"))
    }
}

impl HttpGuard for V2Guard {}

#[gateway(path = "/chat", version = "1")]
#[use_guards(V1Guard)]
struct ChatV1Gateway;

#[messages]
impl ChatV1Gateway {
    #[subscribe_message("say")]
    #[public]
    async fn say(&self) -> String {
        "v1".into()
    }
}

#[gateway(path = "/chat", version = "2")]
#[use_guards(V2Guard)]
struct ChatV2Gateway;

#[messages]
impl ChatV2Gateway {
    #[subscribe_message("say")]
    #[public]
    async fn say(&self) -> String {
        "v2".into()
    }
}

#[module(
    imports = [WsModule],
    providers = [ChatV1Gateway, ChatV2Gateway, V1Guard, V2Guard],
)]
struct TwoVersionsModule;

/// Two gateways, one declared `path`, two versions. The duplicate-self-mount
/// boot error compares the **effective** path, so these are two mounts rather
/// than a collision — and each serves its own upgrade chain, which is what makes
/// them independent rather than merely distinct.
#[tokio::test]
async fn two_versions_of_one_path_both_boot_and_serve_their_own_chain() {
    let app = TestApp::for_module::<TwoVersionsModule>()
        .await
        .expect("one path under two versions is two mounts, not a collision");

    for (path, expected) in [
        ("/v1/chat", "the v1 socket refused you"),
        ("/v2/chat", "the v2 socket refused you"),
    ] {
        let response = app.http().get(path).send().await;
        response.assert_status(StatusCode::UNAUTHORIZED);
        let body = response.0.into_body().into_string().await.expect("a body");
        assert!(
            body.contains(expected),
            "{path} must run its own gateway's guard, got: {body}",
        );
    }

    // The bare path belongs to neither.
    app.http()
        .get("/chat")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);

    // …and the message tables stayed separate, each answering for itself.
    assert!(matches!(
        ChatV1Gateway.dispatch(&WsClient::for_test(), "say", serde_json::Value::Null).await,
        WsReply::Reply(v) if v.as_str() == Some("v1"),
    ));
    assert!(matches!(
        ChatV2Gateway.dispatch(&WsClient::for_test(), "say", serde_json::Value::Null).await,
        WsReply::Reply(v) if v.as_str() == Some("v2"),
    ));
}

#[gateway(path = "/twin", version = "1")]
struct TwinAGateway;

#[messages]
impl TwinAGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

#[gateway(path = "/twin", version = "1")]
struct TwinBGateway;

#[messages]
impl TwinBGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

#[module(imports = [WsModule], providers = [TwinAGateway, TwinBGateway])]
struct TwinModule;

/// The half a version must not weaken: same path *and* same version is still one
/// address with two owners, and it fails boot naming both rather than letting
/// poem panic during route assembly.
#[tokio::test]
async fn one_path_and_one_version_shared_by_two_gateways_still_fails_boot() {
    let Err(err) = TestApp::for_module::<TwinModule>().await else {
        panic!("a gateway owns its mount — two of them cannot own one address");
    };
    let message = format!("{err:#}");
    for owner in ["TwinAGateway", "TwinBGateway"] {
        assert!(
            message.contains(owner),
            "the boot error names both claimants: {message}",
        );
    }
    assert!(
        message.contains("/v1/twin"),
        "…at the address they actually collide on: {message}",
    );
}

// ── Above `dispatch`: the connection itself ─────────────────────────────────
//
// Everything up to here calls `Gateway::dispatch` directly, which is the whole
// of the message table and none of the socket. The upgrade, the `WsConfig` it
// resolves, the socket-lifetime ceiling, the per-message cap, the writer task,
// the registry entry's cleanup and every Close frame the server sends live in
// the connection task `on_upgrade` spawns, and none of them had a witness in
// this workspace. `nest_rs_testing::ws` binds a real port so they do.

use nest_rs_testing::LogCapture;
use nest_rs_testing::ws::WsFrame;
use nest_rs_ws::CloseCode;
use nest_rs_ws::{WsConfig, WsServer};
use std::time::Duration;

/// A read budget for asserting *absence*, so a test does not pay the driver's
/// full timeout to prove a socket stayed quiet.
const QUIET: Duration = Duration::from_millis(150);

#[gateway(path = "/socket")]
pub struct SocketGateway;

#[messages]
impl SocketGateway {
    #[subscribe_message("echo")]
    #[public]
    async fn echo(&self, text: String) -> String {
        text
    }
}

#[module(imports = [WsModule], providers = [SocketGateway])]
struct SocketModule;

/// The round trip nothing asserted: a real handshake, the connection task, the
/// writer half, and back. Below it, the three `nest_rs::operation` lines the
/// edge owes — a socket that opens, serves and closes with nothing on the
/// console is work no operator can account for.
#[tokio::test]
async fn a_message_round_trips_over_a_real_upgrade() {
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::builder()
        .module::<SocketModule>()
        .build_ws()
        .await
        .expect("a gateway boots on a real port");
    let server = app
        .container()
        .get::<WsServer>()
        .expect("WsModule provides the registry");

    let mut socket = app.socket("/socket").connect().await;
    socket.send("echo", serde_json::json!("hi")).await;
    let reply = socket.next_envelope().await;
    assert_eq!(reply["event"], "echo");
    assert_eq!(reply["data"], "hi");
    assert_eq!(
        server.connection_count(),
        1,
        "the upgrade registered the connection",
    );

    // §5.5.1: an endpoint that *receives* a Close must send one back. The
    // protocol layer queues that echo when it decodes the frame — and nothing
    // ever flushed it, because the writer task simply dropped the `Sink`, so
    // the client read 1006 on a close it had itself requested.
    let echo = socket
        .close(CloseCode::Normal, "done")
        .await
        .expect("§5.5.1: a received Close is answered with one");
    assert_eq!(
        echo.0,
        CloseCode::Normal,
        "the peer's own status comes back"
    );

    // The registry entry goes with the connection: `RegistryGuard`'s `Drop`
    // runs before the echo reaches the wire, so observing the echo is enough
    // to observe the cleanup.
    assert_eq!(server.connection_count(), 0, "the entry did not outlive it");

    for unit in [
        nest_rs_ws::unit::CONNECT,
        nest_rs_ws::unit::MESSAGE,
        nest_rs_ws::unit::DISCONNECT,
    ] {
        let line = logs.expect_one(nest_rs_core::operation_log::TARGET, unit);
        assert_eq!(line.message, unit);
        assert!(
            line.field("conn_id").is_some(),
            "{unit} names the connection: {:?}",
            line.fields,
        );
        assert!(
            line.field("duration_ms").is_some(),
            "{unit} is timed: {:?}",
            line.fields,
        );
    }
    let message = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::MESSAGE,
    );
    assert_eq!(message.field("event").as_deref(), Some("echo"));
    assert_eq!(
        message.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::OK),
    );

    app.shutdown().await.expect("the transport stops cleanly");
}

#[module(
    imports = [WsModule, WsModule::for_root(WsConfig {
        max_connection: Some(Duration::from_millis(200)),
        ..WsConfig::default()
    })],
    providers = [SocketGateway],
)]
struct CeilingModule;

/// The ceiling is a security control — it forces a re-upgrade so `exp` is
/// checked again — and a client that cannot tell it from a dropped connection
/// retries blindly and never re-authenticates. It closed by dropping the
/// `Sink`, so the peer read **1006**, which §7.4.1 reserves for a connection
/// closed with no Close frame at all: a network fault.
#[tokio::test]
async fn the_lifetime_ceiling_closes_with_going_away() {
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::builder()
        .module::<CeilingModule>()
        .build_ws()
        .await
        .expect("a pinned ceiling boots");

    let mut socket = app.socket("/socket").connect().await;
    let (code, reason) = socket.expect_close().await;
    assert_eq!(
        code,
        CloseCode::Away,
        "§7.4.1 1001: the server is deliberately ending a socket it will no longer serve",
    );
    assert!(
        !reason.is_empty(),
        "the peer is told what to do about it, not only that it happened",
    );

    // The operator's half of the same event, so a close on the wire and a line
    // in the log cannot come to describe different closes.
    let closing = logs.expect_one("nest_rs::ws", "closing socket: max lifetime reached");
    assert_eq!(
        closing.field("close_code").as_deref(),
        Some(u16::from(CloseCode::Away).to_string().as_str()),
    );

    app.shutdown().await.expect("the transport stops cleanly");
}

#[module(
    imports = [WsModule, WsModule::for_root(WsConfig {
        max_message_bytes: 64,
        ..WsConfig::default()
    })],
    providers = [SocketGateway],
)]
struct CappedModule;

/// The per-message cap is enforced at the protocol layer, so an oversize frame
/// is refused *while reading* (WS-I1) — and the refusal surfaces as a read
/// error, with the framing gone mid-message. The socket cannot continue, so it
/// ends; what changed is that it now ends by *saying so*.
#[tokio::test]
async fn a_message_past_the_cap_closes_the_socket_instead_of_vanishing() {
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::builder()
        .module::<CappedModule>()
        .build_ws()
        .await
        .expect("a pinned message cap boots");

    let mut socket = app.socket("/socket").connect().await;
    socket
        .send("echo", serde_json::json!("x".repeat(512)))
        .await;

    let (code, reason) = socket.expect_close().await;
    assert_eq!(
        code,
        CloseCode::Error,
        "§7.4.1 1011: poem hands the cause on as an opaque `io::Error`, so the cap cannot be \
         claimed as the reason — a code the peer cannot check would be worse than the generic one",
    );
    assert!(!reason.is_empty());

    let read = logs.expect_one("nest_rs::ws", "websocket read error");
    assert!(
        read.field("error")
            .is_some_and(|e| e.contains("Space limit")),
        "…while the operator's line does name the size that did it: {:?}",
        read.fields,
    );

    app.shutdown().await.expect("the transport stops cleanly");
}

/// RFC 6455 §5.6 makes Binary a first-class data frame, so a client is entitled
/// to send one. This gateway's contract is a JSON text envelope, so refusing it
/// is right — refusing it with no reply and no log at any level is the silent
/// failure `CLAUDE.md` forbids, and from the client's side it is
/// indistinguishable from a handler that never answered.
#[tokio::test]
async fn a_binary_frame_is_refused_in_band_and_the_socket_survives() {
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::builder()
        .module::<SocketModule>()
        .build_ws()
        .await
        .expect("a gateway boots on a real port");

    let mut socket = app.socket("/socket").connect().await;
    socket.send_binary(vec![0x00, 0x01, 0x02]).await;

    let refusal = socket.next_envelope().await;
    assert_eq!(refusal["event"], "error");
    assert!(
        refusal["data"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("binary")),
        "the client is told what was wrong with the frame: {refusal}",
    );

    let event = logs.expect_one(
        "nest_rs::ws",
        "websocket message refused: binary frames carry no envelope",
    );
    assert_eq!(event.field("bytes").as_deref(), Some("3"));

    // Framing is intact, so the refusal is in band and the connection lives —
    // the same answer the oversize boundary gives, for the same reason.
    socket.send("echo", serde_json::json!("still here")).await;
    assert_eq!(socket.next_envelope().await["data"], "still here");

    app.shutdown().await.expect("the transport stops cleanly");
}

/// An unrouted event answers and keeps the socket open — the frame is the
/// answer, and nothing follows it. Proves the driver's silence assertion is
/// real, and pins the one arm of the loop that must *not* close.
#[tokio::test]
async fn an_unknown_event_answers_once_and_leaves_the_socket_open() {
    let app = nest_rs_testing::TestApp::builder()
        .module::<SocketModule>()
        .build_ws()
        .await
        .expect("a gateway boots on a real port");

    let mut socket = app.socket("/socket").connect().await;
    socket.send("nope", serde_json::Value::Null).await;
    let frame = socket.next_envelope().await;
    assert!(
        frame["data"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("unknown")),
        "{frame}",
    );
    socket.expect_silence(QUIET).await;
    assert!(
        !matches!(
            socket.next_frame_within(QUIET).await,
            Some(WsFrame::Close(_)),
        ),
        "an unrouted event is a client typo, not a reason to end the connection",
    );

    app.shutdown().await.expect("the transport stops cleanly");
}
