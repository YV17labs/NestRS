//! `#[messages]`-generated `Gateway::dispatch` — the return-type shape paths
//! the macro picks (Unit / Value / `Result<(), E>` / `Result<T, E>`). The macro
//! itself lives in `nest-rs-ws-macros`; this file pins its observable behaviour.

use nest_rs_pipes::{Pipe, PipeError, Piped, Trim, Valid};
use nest_rs_ws::{Gateway, WsClient, WsReply, gateway, messages};
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
    async fn ok_handler(&self) -> Result<String, std::io::Error> {
        Ok("yay".to_string())
    }

    #[subscribe_message("err")]
    async fn err_handler(&self) -> Result<String, std::io::Error> {
        Err(std::io::Error::other("boom"))
    }

    #[subscribe_message("ok_unit")]
    async fn ok_unit_handler(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[subscribe_message("err_unit")]
    async fn err_unit_handler(&self) -> Result<(), std::io::Error> {
        Err(std::io::Error::other("boom-unit"))
    }

    #[subscribe_message("plain")]
    async fn plain_handler(&self) -> String {
        "hello".to_string()
    }

    #[subscribe_message("nothing")]
    async fn nothing_handler(&self) {}

    // `Piped<Trim, String>`: the wire payload is a `String`; the handler sees it
    // trimmed — the WS analog of the HTTP / GraphQL / queue pipe forms.
    #[subscribe_message("trim")]
    async fn trim_handler(&self, name: Piped<Trim, String>) -> String {
        name.into_inner()
    }

    // A rejecting pipe replies with an error frame, never reaching the body.
    #[subscribe_message("checked")]
    async fn checked_handler(&self, name: Piped<Reject, String>) -> String {
        name.into_inner()
    }

    // `Valid<T>`: validates the deserialized payload before the handler runs.
    #[subscribe_message("named")]
    async fn named_handler(&self, input: Valid<NameInput>) -> String {
        input.into_inner().name
    }

    // The two spellings of one type. `literal` is what the macro can see; the
    // three `renamed_*` handlers are the same `Result` behind an alias.
    #[subscribe_message("literal")]
    async fn literal_handler(&self) -> Result<String, DbFailure> {
        Err(failure())
    }

    #[subscribe_message("renamed")]
    async fn renamed_handler(&self) -> ServiceResult<String> {
        Err(failure())
    }

    #[subscribe_message("renamed_ok")]
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
            assert!(msg.contains("boom"), "want 'boom' in {msg}");
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
            assert!(msg.contains("boom-unit"), "want 'boom-unit' in {msg}");
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
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "missing", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Error(msg) => {
            assert!(
                msg.contains("missing") && msg.contains("unknown"),
                "want 'unknown' + the event name in {msg}",
            );
        }
        _ => panic!("expected Error for an unrouted event"),
    }
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
        WsReply::Error(msg) => assert!(msg.contains("bad input"), "want 'bad input' in {msg}"),
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
                msg.contains("validation failed"),
                "want validation error in {msg}"
            )
        }
        _ => panic!("expected a validation error frame"),
    }
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
        WsReply::Error(msg) => assert_eq!(msg, "database unavailable"),
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
            WsReply::Error(msg) => frames.push(msg),
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
