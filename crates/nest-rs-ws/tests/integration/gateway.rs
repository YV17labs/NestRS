//! `#[messages]`-generated `Gateway::dispatch` — its return-type shapes — and
//! the address the generated `Discoverable` mounts it at.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_pipes::{ParseArray, Pipe, PipeError, Piped, Trim, Valid};
use nest_rs_testing::TestApp;
use nest_rs_ws::nest_rs_http::poem::Request as HttpRequest;
use nest_rs_ws::{Gateway, WsClient, WsModule, WsReply, async_trait, gateway, messages};
use poem::http::{StatusCode, header};
use serde::{Deserialize, Serialize};
use validator::Validate;

/// A typed error that is `Serialize` and whose `Display` withholds a field.
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

/// The alias the syntactic return-type detection cannot see through.
type ServiceResult<T> = Result<T, DbFailure>;

/// The same shape with an error that is `Display` and **not** `Error`.
#[derive(Debug, Serialize)]
struct DisplayOnly {
    dsn: String,
}

impl std::fmt::Display for DisplayOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("display-only failure")
    }
}

type DisplayOnlyResult<T> = Result<T, DisplayOnly>;

/// A pipe that always rejects.
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

/// A signup a handler validates itself, the way its service does.
#[derive(Validate)]
struct Signup {
    #[validate(length(min = 32))]
    password: String,
}

/// `ServiceError::Validation`'s shape: a constant sentence, the validation
/// failure kept as its source.
#[derive(Debug, thiserror::Error)]
enum SignupError {
    #[error("validation failed")]
    Validation(#[from] validator::ValidationErrors),
}

/// A developer's own error saying the validation failure as its own.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
struct SpelledSignupError(#[from] validator::ValidationErrors);

#[gateway(path = "/test")]
pub(crate) struct TestGateway;

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

    #[subscribe_message("trim")]
    #[public]
    async fn trim_handler(&self, name: Piped<Trim, String>) -> String {
        name.into_inner()
    }

    #[subscribe_message("checked")]
    #[public]
    async fn checked_handler(&self, name: Piped<Reject, String>) -> String {
        name.into_inner()
    }

    // A list whose items must parse: a refused item is said, never quoted.
    #[subscribe_message("ids")]
    #[public]
    async fn ids_handler(&self, ids: Piped<ParseArray<u64>, String>) -> Vec<u64> {
        ids.into_inner()
    }

    // A handler whose own validation refuses the payload, through its error.
    #[subscribe_message("sign_up")]
    #[public]
    async fn sign_up_handler(&self, password: String) -> Result<String, SignupError> {
        Signup { password }.validate()?;
        Ok("signed up".to_owned())
    }

    // The same refusal, through an error that spells it.
    #[subscribe_message("sign_up_spelled")]
    #[public]
    async fn sign_up_spelled_handler(
        &self,
        password: String,
    ) -> Result<String, SpelledSignupError> {
        Signup { password }.validate()?;
        Ok("signed up".to_owned())
    }

    #[subscribe_message("named")]
    #[public]
    async fn named_handler(&self, input: Valid<NameInput>) -> String {
        input.into_inner().name
    }

    // `literal` is what the macro can see; `renamed_*` hide the `Result` behind an alias.
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

    #[subscribe_message("renamed_display_only")]
    #[public]
    async fn renamed_display_only_handler(&self) -> DisplayOnlyResult<String> {
        Err(DisplayOnly {
            dsn: "postgres://u:hunter2@db".to_string(),
        })
    }

    #[subscribe_message("renamed_ok")]
    #[public]
    async fn renamed_ok_handler(&self) -> ServiceResult<String> {
        Ok("fine".to_string())
    }

    // A `Result` around a `Result`: the inner `Err` is the handler's failure,
    // spelled literally or through the alias.
    #[subscribe_message("nested_literal")]
    #[public]
    async fn nested_literal_handler(&self) -> Result<Result<String, DbFailure>, std::io::Error> {
        Ok(Err(failure()))
    }

    #[subscribe_message("nested_renamed")]
    #[public]
    async fn nested_renamed_handler(&self) -> ServiceResult<Result<String, DbFailure>> {
        Ok(Err(failure()))
    }

    #[subscribe_message("nested_ok")]
    #[public]
    async fn nested_ok_handler(&self) -> Result<ServiceResult<String>, std::io::Error> {
        Ok(Ok("fine".to_string()))
    }

    // Errors that are not `Send + Sync`, each with a cause beneath its sentence.
    #[subscribe_message("unsendable_boxed")]
    #[public]
    async fn unsendable_boxed_handler(&self) -> Result<String, Box<dyn std::error::Error>> {
        Err(Box::new(LocalFailure::new()))
    }

    #[subscribe_message("unsendable_rc")]
    #[public]
    async fn unsendable_rc_handler(&self) -> Result<String, LocalFailure> {
        Err(LocalFailure::new())
    }

    // A body the handler decodes itself, failing in each tier's shape.
    #[subscribe_message("decode_anyhow")]
    #[public]
    async fn decode_anyhow_handler(&self) -> nest_rs_core::anyhow::Result<String> {
        let amount: u64 = serde_json::from_str(SECRET_BODY)?;
        Ok(amount.to_string())
    }

    #[subscribe_message("decode_serde")]
    #[public]
    async fn decode_serde_handler(&self) -> Result<String, serde_json::Error> {
        serde_json::from_str::<u64>(SECRET_BODY).map(|amount| amount.to_string())
    }

    #[subscribe_message("decode_unsendable")]
    #[public]
    async fn decode_unsendable_handler(&self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(serde_json::from_str::<u64>(SECRET_BODY)?.to_string())
    }

    #[subscribe_message("decode_display_only")]
    #[public]
    async fn decode_display_only_handler(&self) -> Result<String, DecodeDisplayOnly> {
        serde_json::from_str::<u64>(SECRET_BODY)
            .map(|amount| amount.to_string())
            .map_err(DecodeDisplayOnly)
    }

    /// The edge's own deliberate error.
    #[subscribe_message("deliberate")]
    #[public]
    async fn deliberate_handler(&self) -> Result<String, nest_rs_ws::WsError> {
        Err(nest_rs_ws::WsError::new("nope"))
    }

    /// A domain error saying what its client may read.
    #[subscribe_message("taken")]
    #[public]
    async fn taken_handler(&self) -> Result<String, Taken> {
        Err(Taken)
    }

    /// A `Problem` carried through anyhow answers itself.
    #[subscribe_message("problem_in_anyhow")]
    #[public]
    async fn problem_in_anyhow_handler(&self) -> nest_rs_core::anyhow::Result<String> {
        Err(
            nest_rs_core::anyhow::Error::from(nest_rs_core::Problem::new(
                404,
                nest_rs_core::problem::code::NOT_FOUND,
            ))
            .context("loading the room"),
        )
    }

    /// A deliberate frame, carried through anyhow: it is found and sent whole,
    /// details included.
    #[subscribe_message("frame_in_anyhow")]
    #[public]
    async fn frame_in_anyhow_handler(&self) -> nest_rs_core::anyhow::Result<String> {
        Err(nest_rs_ws::WsError::with_details(
            "pick another name",
            serde_json::json!({ "name": ["taken"] }),
        )
        .into())
    }
}

/// A domain error that says what its client may read, never its own sentence.
#[derive(Debug, thiserror::Error)]
#[error("the handle ada@example.com is taken")]
struct Taken;

impl nest_rs_core::ToProblem for Taken {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        Some(
            nest_rs_core::Problem::new(409, nest_rs_core::problem::code::CONFLICT)
                .with_detail("the handle is taken")
                .with_retry_after(30),
        )
    }
}

/// What every `decode_*` handler decodes: a number, sent a secret.
const SECRET_BODY: &str = r#""sk_live_51HsecretTOKEN""#;

/// A `Display`-only error spelling a decode failure — the third tier, which has
/// no chain to read vocabulary off: it answers opaquely, its sentence on the
/// operator's line said without the value.
struct DecodeDisplayOnly(serde_json::Error);

impl std::fmt::Display for DecodeDisplayOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not read the amount: {}", self.0)
    }
}

/// An error holding an `Rc` — neither `Send` nor `Sync` — whose sentence names
/// no cause, so only a walk of its chain reaches the one beneath it.
#[derive(Debug)]
struct LocalFailure {
    cause: std::rc::Rc<std::io::Error>,
}

impl LocalFailure {
    fn new() -> Self {
        Self {
            cause: std::rc::Rc::new(std::io::Error::other("replica lagging")),
        }
    }
}

impl std::fmt::Display for LocalFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("read failed")
    }
}

impl std::error::Error for LocalFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.cause)
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

/// An error nobody meant for the client answers the opaque frame; its chain is
/// filed once, at `error`.
#[tokio::test]
async fn result_err_becomes_an_opaque_error_frame() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "err", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Error(msg) => {
            assert_eq!(msg.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE, "{msg}");
            assert!(msg.errors.is_none(), "{msg:?}");
        }
        _ => panic!("expected Error for Result::Err"),
    }
    let failed = logs.expect_one(nest_rs_ws::TARGET, "websocket message failed");
    assert_eq!(failed.level, "error");
    assert_eq!(failed.field("event").as_deref(), Some("err"));
    assert_eq!(failed.field("error").as_deref(), Some("boom"));
    assert!(
        logs.find(nest_rs_ws::TARGET, "subscribe_message handler returned Err")
            .is_empty(),
        "an opaque failure is filed once",
    );
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
async fn result_err_unit_becomes_an_opaque_error_frame() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "err_unit", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Error(msg) => assert_eq!(msg.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE),
        _ => panic!("expected Error for Result<(), E>::Err"),
    }
    let failed = logs.expect_one(nest_rs_ws::TARGET, "websocket message failed");
    assert_eq!(failed.field("error").as_deref(), Some("boom-unit"));
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

#[tokio::test]
async fn an_aliased_result_produces_an_error_frame_not_a_serialized_err() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "renamed", serde_json::Value::Null)
        .await;

    match reply {
        WsReply::Error(msg) => assert_eq!(msg.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE),
        WsReply::Reply(value) => panic!("the Err variant was shipped as a success frame — {value}"),
        WsReply::None => panic!("expected an error frame"),
    }

    let event = logs.expect_one("nest_rs::ws", "websocket message failed");
    assert_eq!(event.level, "error");
    assert_eq!(event.field("event").as_deref(), Some("renamed"));
    assert_eq!(
        event.field("error").as_deref(),
        Some("database unavailable")
    );
}

#[tokio::test]
async fn an_aliased_result_with_a_display_only_error_is_an_error_frame() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "renamed_display_only",
            serde_json::Value::Null,
        )
        .await;

    match reply {
        WsReply::Error(msg) => assert_eq!(msg.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE),
        WsReply::Reply(value) => panic!("the Err variant was shipped as a success frame — {value}"),
        WsReply::None => panic!("expected an error frame"),
    }
    let failed = logs.expect_one("nest_rs::ws", "websocket message failed");
    assert_eq!(
        failed.field("error").as_deref(),
        Some("display-only failure")
    );
}

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
    assert!(!frames[0].contains("hunter2"), "{}", frames[0]);
}

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

#[tokio::test]
async fn a_result_inside_a_result_is_an_error_frame_however_it_is_spelled() {
    for event in ["nested_literal", "nested_renamed"] {
        let logs = nest_rs_testing::LogCapture::install();
        match TestGateway
            .dispatch(&WsClient::for_test(), event, serde_json::Value::Null)
            .await
        {
            WsReply::Error(msg) => {
                assert_eq!(msg.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE, "{event}");
            }
            WsReply::Reply(value) => {
                panic!("`{event}`: the inner Err was shipped as a success frame — {value}")
            }
            WsReply::None => panic!("`{event}`: expected an error frame"),
        }
        let line = logs.expect_one("nest_rs::ws", "websocket message failed");
        assert_eq!(line.field("event").as_deref(), Some(event));
        assert_eq!(line.field("error").as_deref(), Some("database unavailable"));
    }
}

#[tokio::test]
async fn a_result_inside_a_result_replies_with_the_inner_value() {
    match TestGateway
        .dispatch(&WsClient::for_test(), "nested_ok", serde_json::Value::Null)
        .await
    {
        WsReply::Reply(v) => assert_eq!(v.as_str(), Some("fine")),
        _ => panic!("expected the inner Ok value"),
    }
}

#[tokio::test]
async fn an_error_that_is_not_send_logs_its_cause_chain() {
    for event in ["unsendable_boxed", "unsendable_rc"] {
        let logs = nest_rs_testing::LogCapture::install();
        match TestGateway
            .dispatch(&WsClient::for_test(), event, serde_json::Value::Null)
            .await
        {
            WsReply::Error(msg) => {
                assert_eq!(msg.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE, "{event}");
            }
            _ => panic!("`{event}`: expected an error frame"),
        }
        let line = logs.expect_one("nest_rs::ws", "websocket message failed");
        assert_eq!(
            line.field("error").as_deref(),
            Some("read failed: replica lagging"),
            "`{event}` logs the whole chain",
        );
    }
}

/// `/websockets/messages/` promises a `warn!` on `nest_rs::ws` for every denied
/// dispatch.
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

#[tokio::test]
async fn a_malformed_payload_is_reported_without_its_value() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "named",
            serde_json::json!({ "name": 4_242_424_242_424_242_u64 }),
        )
        .await;
    let WsReply::Error(frame) = reply else {
        panic!("expected an error frame");
    };
    assert_eq!(
        frame.error,
        "invalid payload for `named`: invalid type: an integer, expected a string"
    );
    let event = logs.expect_one(
        "nest_rs::ws",
        "subscribe_message payload failed to deserialize",
    );
    assert_eq!(
        event.field("error").as_deref(),
        Some("invalid type: an integer, expected a string")
    );
}

#[tokio::test]
async fn a_refused_list_item_is_quoted_neither_in_the_frame_nor_on_the_line() {
    let logs = nest_rs_testing::LogCapture::install();
    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "ids",
            serde_json::json!("1,sk_live_51HsecretTOKEN,3"),
        )
        .await;
    let WsReply::Error(error) = reply else {
        panic!("expected an error frame from the refusing pipe");
    };
    let frame = nest_rs_ws::WsEnvelope::encode("ids", &error).expect("encode");
    assert!(
        !frame.contains("sk_live"),
        "the frame quotes the item: {frame}"
    );
    logs.expect_one(nest_rs_ws::TARGET, "subscribe_message rejected by a pipe");
    let quoting: Vec<String> = logs
        .events()
        .into_iter()
        .filter(|event| event.fields.values().any(|value| value.contains("sk_live")))
        .map(|event| format!("{} {:?}", event.message, event.fields))
        .collect();
    assert!(quoting.is_empty(), "lines quoting the item: {quoting:#?}");
}

/// validator's own `Display` prints every rule's parameters, the submitted
/// value among them.
#[tokio::test]
async fn a_validation_failure_a_handler_returns_is_said_without_the_submitted_value() {
    let mut quoting = Vec::new();
    for event in ["sign_up", "sign_up_spelled"] {
        let logs = nest_rs_testing::LogCapture::install();
        let reply = TestGateway
            .dispatch(
                &WsClient::for_test(),
                event,
                serde_json::json!("sk_live_51HsecretTOKEN"),
            )
            .await;
        let WsReply::Error(error) = reply else {
            panic!("`{event}`: expected an error frame from the refusing handler");
        };
        let frame = nest_rs_ws::WsEnvelope::encode(event, &error).expect("encode");
        if frame.contains("sk_live") {
            quoting.push(format!("`{event}` frame: {frame}"));
        }
        assert_eq!(error.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE, "{event}");
        logs.expect_one(nest_rs_ws::TARGET, "websocket message failed");
        quoting.extend(
            logs.events()
                .into_iter()
                .filter(|line| line.fields.values().any(|value| value.contains("sk_live")))
                .map(|line| format!("`{event}` line: {} {:?}", line.message, line.fields)),
        );
    }
    assert!(
        quoting.is_empty(),
        "the submitted value, quoted: {quoting:#?}"
    );
}

#[gateway(path = "/ws", version = "1")]
pub(crate) struct VersionedGateway;

#[messages]
impl VersionedGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        "v1".into()
    }
}

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

#[test]
fn an_undeclared_version_leaves_the_mount_alone() {
    assert_eq!(TestGateway::VERSION, None);
    assert_eq!(TestGateway::__nestrs_mount_path(), "/test");
}

#[module(imports = [WsModule], providers = [VersionedGateway])]
struct VersionedModule;

/// The status an in-process handshake gets: `500 no upgrade` is poem's
/// `WebSocket` extractor accepting the whole handshake and finding no hyper
/// upgrade seam, which only a mounted gateway answers.
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

    assert_eq!(upgrade_status(&app, "/ws").await, StatusCode::NOT_FOUND);
    app.http()
        .get("/ws")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

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

// Each version's connection guard denies with its own reason, so the response
// names the gateway whose upgrade chain ran.

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

    app.http()
        .get("/chat")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);

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

/// Fails boot naming both rather than letting poem panic during route assembly.
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

use nest_rs_testing::LogCapture;
use nest_rs_testing::ws::WsFrame;
use nest_rs_ws::CloseCode;
use nest_rs_ws::{WsConfig, WsServer};
use std::time::Duration;

/// A read budget for asserting absence, shorter than the driver's full timeout.
const QUIET: Duration = Duration::from_millis(150);

#[gateway(path = "/socket")]
pub(crate) struct SocketGateway;

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

    let echo = socket
        .close(CloseCode::Normal, "done")
        .await
        .expect("§5.5.1: a received Close is answered with one");
    assert_eq!(
        echo.0,
        CloseCode::Normal,
        "the peer's own status comes back"
    );

    // `RegistryGuard`'s `Drop` runs before the echo reaches the wire.
    assert_eq!(server.connection_count(), 0, "the entry did not outlive it");

    for unit in [
        nest_rs_ws::unit::CONNECT.name(),
        nest_rs_ws::unit::MESSAGE.name(),
        nest_rs_ws::unit::DISCONNECT.name(),
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
        nest_rs_ws::unit::MESSAGE.name(),
    );
    assert_eq!(message.field("event").as_deref(), Some("echo"));
    assert_eq!(
        message.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::OK),
    );

    app.shutdown().await.expect("the transport stops cleanly");
}

/// The floor every connection ceiling is held to: a second.
const CEILING: Duration = Duration::from_secs(1);

#[module(
    imports = [WsModule, WsModule::for_root(WsConfig {
        max_connection: Some(CEILING),
        ..WsConfig::default()
    })],
    providers = [SocketGateway],
)]
struct CeilingModule;

#[tokio::test]
async fn the_lifetime_ceiling_closes_with_going_away() {
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::builder()
        .module::<CeilingModule>()
        .build_ws()
        .await
        .expect("a pinned ceiling boots");

    let mut socket = app.socket("/socket").connect().await;
    // The ceiling passes on paused time, while nothing reads: its Close waits
    // buffered for the read below, back on the real clock.
    tokio::time::pause();
    tokio::time::sleep(CEILING + Duration::from_millis(1)).await;
    tokio::time::resume();
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

/// The protocol-layer cap surfaces as a read error with the framing gone, so the
/// socket cannot continue.
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

    socket.send("echo", serde_json::json!("still here")).await;
    assert_eq!(socket.next_envelope().await["data"], "still here");

    app.shutdown().await.expect("the transport stops cleanly");
}

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

// An attribute macro reads a method before its `#[cfg]` is evaluated, so every
// hook reaches the expansion, including one compiled out.

static CFG_HOOK_RAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[gateway(path = "/cfg-hooks")]
pub(crate) struct CfgHookGateway;

#[messages]
impl CfgHookGateway {
    #[cfg(test)]
    #[on_connect]
    fn connected(&self) {
        CFG_HOOK_RAN.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(any())]
    #[on_connect]
    fn never_compiled(&self) {}
}

#[tokio::test]
async fn a_hook_compiled_in_survives_a_compiled_out_one_declared_after_it() {
    CfgHookGateway.on_connect(&WsClient::for_test()).await;
    assert!(
        CFG_HOOK_RAN.load(std::sync::atomic::Ordering::SeqCst),
        "the `#[on_connect]` whose `#[cfg]` holds is the one the gateway runs",
    );
}

/// Every tier a handler's error takes: `anyhow::Result` with `?`, serde's error
/// itself, an error that is not `Send`, and a `Display`-only one.
#[tokio::test]
async fn a_decode_failure_a_handler_returns_is_framed_and_logged_without_its_value() {
    const REPORT: &str = "invalid type: a string, expected u64 at line 1 column 24";
    let logs = nest_rs_testing::LogCapture::install();
    for (event, framed) in [
        ("decode_anyhow", REPORT.to_owned()),
        ("decode_serde", REPORT.to_owned()),
        ("decode_unsendable", REPORT.to_owned()),
        (
            "decode_display_only",
            nest_rs_core::OPAQUE_CLIENT_MESSAGE.to_owned(),
        ),
    ] {
        let reply = TestGateway
            .dispatch(&WsClient::for_test(), event, serde_json::Value::Null)
            .await;
        let WsReply::Error(frame) = reply else {
            panic!("{event}: a failed handler answers with an error frame");
        };
        assert_eq!(frame.error, framed, "{event}");
    }
    let quoting: Vec<String> = logs
        .events()
        .into_iter()
        .filter(|event| event.fields.values().any(|value| value.contains("sk_live")))
        .map(|event| format!("{} {:?}", event.message, event.fields))
        .collect();
    assert!(quoting.is_empty(), "lines quoting the value: {quoting:#?}");
}

/// The edge's own deliberate error answers as built.
#[tokio::test]
async fn a_ws_error_a_handler_returns_answers_as_built() {
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "deliberate", serde_json::Value::Null)
        .await;
    let WsReply::Error(frame) = reply else {
        panic!("a failed handler answers with an error frame");
    };
    assert_eq!(frame.error, "nope");
    assert!(frame.errors.is_none(), "{frame:?}");
}

/// A `ToProblem` error answers its problem — its detail, its code and its wait
/// under `errors` — never its own sentence; a `Problem` in an anyhow chain
/// answers itself.
#[tokio::test]
async fn a_problem_answers_its_detail_its_code_and_its_wait() {
    let reply = TestGateway
        .dispatch(&WsClient::for_test(), "taken", serde_json::Value::Null)
        .await;
    let WsReply::Error(frame) = reply else {
        panic!("a failed handler answers with an error frame");
    };
    assert_eq!(frame.error, "the handle is taken");
    assert_eq!(
        frame.errors,
        Some(serde_json::json!({ "code": "CONFLICT", "retryAfterSeconds": 30 })),
    );

    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "problem_in_anyhow",
            serde_json::Value::Null,
        )
        .await;
    let WsReply::Error(frame) = reply else {
        panic!("a failed handler answers with an error frame");
    };
    assert_eq!(frame.error, "not found");
    assert_eq!(
        frame.errors,
        Some(serde_json::json!({ "code": "NOT_FOUND" }))
    );
}

#[tokio::test]
async fn a_frame_returned_through_anyhow_is_sent_whole() {
    let reply = TestGateway
        .dispatch(
            &WsClient::for_test(),
            "frame_in_anyhow",
            serde_json::Value::Null,
        )
        .await;
    let WsReply::Error(frame) = reply else {
        panic!("a failed handler answers with an error frame");
    };
    assert_eq!(frame.error, "pick another name");
    assert_eq!(frame.errors, Some(serde_json::json!({ "name": ["taken"] })));
}

static SLOW_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();
static STUCK_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();
/// Told once the flood is queued: from there the writer has more to send than
/// any socket buffer holds, so it parks on the first frame the kernel refuses.
static FLOODED: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[gateway(path = "/leaving")]
pub(crate) struct LeavingGateway;

#[messages]
impl LeavingGateway {
    #[subscribe_message("echo")]
    #[public]
    async fn echo(&self, text: String) -> String {
        text
    }

    /// Takes long enough for shutdown to be asked for while it runs, and far
    /// less than the window.
    #[subscribe_message("slow")]
    #[public]
    async fn slow(&self, text: String) -> String {
        SLOW_STARTED.notify_one();
        tokio::time::sleep(Duration::from_millis(300)).await;
        text
    }

    /// Waits on something that never comes.
    #[subscribe_message("stuck")]
    #[public]
    async fn stuck(&self) -> String {
        STUCK_STARTED.notify_one();
        std::future::pending::<()>().await;
        "never".to_owned()
    }

    #[subscribe_message("explode")]
    #[public]
    async fn explode(&self) -> String {
        tokio::task::yield_now().await;
        panic!("the handler exploded with sk_live_secret in hand")
    }

    /// Queues more than a socket's buffers hold, for a client that will not
    /// read it: the writer parks on the first frame the kernel cannot take.
    #[subscribe_message("flood")]
    #[public]
    async fn flood(&self, client: &WsClient) {
        let chunk = "x".repeat(128 * 1024);
        for _ in 0..256 {
            let _ = client.emit("chunk", &chunk);
        }
        FLOODED.notify_one();
    }
}

/// The shutdown window of [`LeavingModule`]'s transport.
const WINDOW: Duration = Duration::from_secs(1);

#[module(
    imports = [
        WsModule,
        nest_rs_ws::nest_rs_http::HttpModule::for_root(nest_rs_ws::nest_rs_http::HttpConfig {
            shutdown_timeout: WINDOW,
            ..Default::default()
        }),
    ],
    providers = [LeavingGateway],
)]
struct LeavingModule;

async fn leaving_app() -> nest_rs_testing::ws::WsApp {
    nest_rs_testing::TestApp::builder()
        .module::<LeavingModule>()
        .build_ws()
        .await
        .expect("a gateway boots on a real port")
}

#[tokio::test]
async fn an_idle_socket_is_closed_going_away_at_the_signal() {
    let logs = LogCapture::install();
    let app = leaving_app().await;
    let mut socket = app.socket("/leaving").connect().await;
    socket.send("echo", serde_json::json!("hi")).await;
    assert_eq!(socket.next_envelope().await["data"], "hi");

    let asked = std::time::Instant::now();
    let stopping = tokio::spawn(app.shutdown());
    let (code, reason) = socket.expect_close().await;
    stopping
        .await
        .expect("shutdown does not panic")
        .expect("the transport stops cleanly");

    assert_eq!(
        code,
        CloseCode::Away,
        "§7.4.1 1001: the server is going down"
    );
    assert!(
        reason.contains("reconnect"),
        "the peer is told what to do: {reason}"
    );
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "an idle socket does not spend the window, took {:?}",
        asked.elapsed(),
    );
    let left = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::DISCONNECT.name(),
    );
    assert_eq!(
        left.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::OK),
        "the disconnect hook still ran: {left:#?}",
    );
}

#[tokio::test]
async fn a_message_running_at_the_signal_is_answered_before_the_close() {
    let app = leaving_app().await;
    let mut socket = app.socket("/leaving").connect().await;
    socket.send("slow", serde_json::json!("last word")).await;
    SLOW_STARTED.notified().await;

    let stopping = tokio::spawn(app.shutdown());
    let reply = socket.next_envelope().await;
    let (code, _) = socket.expect_close().await;
    stopping
        .await
        .expect("shutdown does not panic")
        .expect("the transport stops cleanly");

    assert_eq!(reply["data"], "last word", "the reply came first");
    assert_eq!(code, CloseCode::Away);
}

#[tokio::test]
async fn a_message_still_running_at_the_window_is_dropped_and_files_cancelled() {
    let logs = LogCapture::install();
    let app = leaving_app().await;
    let mut socket = app.socket("/leaving").connect().await;
    socket.send("stuck", serde_json::Value::Null).await;
    STUCK_STARTED.notified().await;

    // The window passes on paused time: nothing reads the socket meanwhile.
    tokio::time::pause();
    let asked = tokio::time::Instant::now();
    app.shutdown().await.expect("the transport stops cleanly");
    let took = asked.elapsed();
    tokio::time::resume();

    assert!(
        took >= WINDOW
            && took < WINDOW + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT + Duration::from_secs(1),
        "the message held the window, and nothing past it — took {took:?}",
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::MESSAGE.name(),
    );
    assert_eq!(line.field("event").as_deref(), Some("stuck"));
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
    assert!(
        matches!(
            socket.read_within(Duration::from_secs(2)).await,
            nest_rs_testing::ws::WsRead::Aborted(_)
        ),
        "the socket was cut with the message",
    );
}

#[tokio::test]
async fn a_message_handler_that_panics_files_panic_and_its_client_is_answered() {
    let logs = LogCapture::install();
    let app = leaving_app().await;
    let mut socket = app.socket("/leaving").connect().await;

    socket.send("explode", serde_json::Value::Null).await;
    let answer = socket.next_envelope().await;
    assert_eq!(answer["event"], "explode");
    assert_eq!(
        answer["data"]["error"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{answer}"
    );
    assert!(!answer.to_string().contains("sk_live"), "{answer}");

    socket.send("echo", serde_json::json!("still here")).await;
    assert_eq!(
        socket.next_envelope().await["data"],
        "still here",
        "the socket goes on serving",
    );

    let exploded: Vec<_> = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_ws::unit::MESSAGE.name(),
        )
        .into_iter()
        .filter(|line| line.field("event").as_deref() == Some("explode"))
        .collect();
    assert_eq!(exploded.len(), 1, "{exploded:#?}");
    assert_eq!(
        exploded[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
    );
    let span = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_ws::unit::MESSAGE.name()
                && span.field("ws.event").as_deref() == Some("explode")
        })
        .expect("the message's span");
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
    let contained = logs.expect_one(
        nest_rs_ws::TARGET,
        "websocket handler panicked; its client is answered with an internal error",
    );
    assert_eq!(contained.level, "error");
    assert!(
        contained
            .field(nest_rs_core::panic::FIELD)
            .is_some_and(|panic| panic.contains("the handler exploded")),
        "{contained:#?}",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[gateway(path = "/broken-hooks")]
pub(crate) struct BrokenHooksGateway;

#[messages]
impl BrokenHooksGateway {
    #[subscribe_message("echo")]
    #[public]
    async fn echo(&self, text: String) -> String {
        text
    }

    #[on_disconnect]
    async fn left(&self) {
        panic!("the disconnect hook exploded")
    }
}

#[gateway(path = "/broken-connect")]
pub(crate) struct BrokenConnectGateway;

#[messages]
impl BrokenConnectGateway {
    #[subscribe_message("echo")]
    #[public]
    async fn echo(&self, text: String) -> String {
        text
    }

    #[on_connect]
    async fn joined(&self) {
        panic!("the connect hook exploded")
    }
}

#[module(imports = [WsModule], providers = [BrokenHooksGateway, BrokenConnectGateway])]
struct BrokenHooksModule;

#[tokio::test]
async fn a_connect_hook_that_panics_files_panic_and_closes_with_internal_error() {
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::builder()
        .module::<BrokenHooksModule>()
        .build_ws()
        .await
        .expect("the gateways boot");

    let mut socket = app.socket("/broken-connect").connect().await;
    let (code, _) = socket.expect_close().await;
    assert_eq!(code, CloseCode::Error, "§7.4.1 1011");
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::CONNECT.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
    );
    let contained = logs.expect_one(
        nest_rs_ws::TARGET,
        "websocket connect hook panicked; the socket is closed unserved",
    );
    assert_eq!(contained.level, "error");
    assert!(
        contained
            .field(nest_rs_core::panic::FIELD)
            .is_some_and(|panic| panic.contains("the connect hook exploded")),
        "{contained:#?}",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_disconnect_hook_that_panics_files_panic_and_the_close_still_completes() {
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::builder()
        .module::<BrokenHooksModule>()
        .build_ws()
        .await
        .expect("the gateways boot");

    let mut socket = app.socket("/broken-hooks").connect().await;
    let echo = socket.close(CloseCode::Normal, "done").await;
    assert_eq!(
        echo.map(|(code, _)| code),
        Some(CloseCode::Normal),
        "§5.5.1: the close is still answered",
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::DISCONNECT.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
    );
    let contained = logs.expect_one(
        nest_rs_ws::TARGET,
        "websocket disconnect hook panicked; the close goes on",
    );
    assert_eq!(contained.level, "error");
    assert!(
        contained
            .field(nest_rs_core::panic::FIELD)
            .is_some_and(|panic| panic.contains("the disconnect hook exploded")),
        "{contained:#?}",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_socket_whose_peer_stopped_reading_goes_with_its_connection() {
    let logs = LogCapture::install();
    let app = leaving_app().await;
    let mut socket = app.socket("/leaving").connect().await;
    socket.send("flood", serde_json::Value::Null).await;
    // The paused clock moves as soon as the runtime idles, which a frame still
    // crossing loopback does not prevent: wait for the handler first.
    FLOODED.notified().await;
    tokio::time::pause();
    tokio::time::sleep(Duration::from_millis(500)).await;
    app.shutdown().await.expect("the transport stops cleanly");
    tokio::time::resume();

    let cut = logs.expect_one(
        nest_rs_ws::nest_rs_http::target::HTTP,
        "connections still open as the shutdown window closes are cut; a request still \
         running is dropped unanswered and a stream ends mid-flow",
    );
    assert_eq!(
        cut.field("upgraded_open").as_deref(),
        Some("0"),
        "no socket outlived the transport: {cut:#?}",
    );
    assert_eq!(cut.field("cut").as_deref(), Some("1"), "{cut:#?}");
    drop(socket);
}

/// The send deadline of [`StallingModule`]'s transport.
const SEND_DEADLINE: Duration = Duration::from_secs(2);

#[module(
    imports = [
        WsModule,
        nest_rs_ws::nest_rs_http::HttpModule::for_root(nest_rs_ws::nest_rs_http::HttpConfig {
            send_timeout: SEND_DEADLINE,
            ..Default::default()
        }),
    ],
    providers = [LeavingGateway],
)]
struct StallingModule;

/// A socket whose client stops reading while the gateway pushes goes at the
/// send deadline, on its own: the read loop ends with the writer, the
/// disconnect hook runs, and no shutdown is needed to free it.
#[tokio::test]
async fn a_socket_whose_client_stops_reading_is_closed_at_the_send_deadline() {
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::builder()
        .module::<StallingModule>()
        .build_ws()
        .await
        .expect("a gateway boots on a real port");
    let mut socket = app.socket("/leaving").connect().await;
    socket.send("flood", serde_json::Value::Null).await;
    FLOODED.notified().await;

    tokio::time::pause();
    tokio::time::sleep(3 * SEND_DEADLINE).await;
    tokio::time::resume();
    nest_rs_testing::wait_until(Duration::from_secs(5), || {
        !logs
            .find(
                nest_rs_core::operation_log::TARGET,
                nest_rs_ws::unit::DISCONNECT.name(),
            )
            .is_empty()
    })
    .await;

    let stalled = logs.expect_one(
        nest_rs_ws::TARGET,
        "the peer took nothing within the send deadline; it is cut off",
    );
    assert_eq!(stalled.level, "warn");
    assert!(stalled.field("conn_id").is_some(), "{stalled:#?}");
    assert!(stalled.field("error").is_some(), "{stalled:#?}");
    let left = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_ws::unit::DISCONNECT.name(),
    );
    assert_eq!(
        left.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::OK),
        "the disconnect hook ran: {left:#?}",
    );
    app.shutdown().await.expect("the transport stops cleanly");
    logs.expect_none(
        nest_rs_ws::nest_rs_http::target::HTTP,
        "connections still open as the shutdown window closes are cut; a request still \
         running is dropped unanswered and a stream ends mid-flow",
    );
    drop(socket);
}

/// Refuses every message, and declares no `WsGuard`: the global pool is
/// `Arc<dyn Guard>`, with no marker to consult.
#[injectable]
#[derive(Default)]
struct UnmarkedWsGuard;

impl Layer for UnmarkedWsGuard {}

#[async_trait]
impl Guard for UnmarkedWsGuard {
    async fn check_ws_message(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &serde_json::Value,
    ) -> Result<(), Denial> {
        Err(Denial::forbidden("refused by an unmarked WS check"))
    }
}

#[module(imports = [WsModule], providers = [SocketGateway, UnmarkedWsGuard])]
struct UnmarkedPoolModule;

/// The other edges are held in `nest-rs-testing`'s `guard_markers`.
#[tokio::test]
async fn a_pooled_guard_without_ws_guard_still_checks_every_message() {
    let app = nest_rs_testing::TestApp::builder()
        .module::<UnmarkedPoolModule>()
        .use_guards_global([nest_rs_guards::guard::<UnmarkedWsGuard>()])
        .build_ws()
        .await
        .expect("an unmarked global guard boots");

    let mut socket = app.socket("/socket").connect().await;
    socket.send("echo", serde_json::json!("hi")).await;
    let reply = socket.next_envelope().await.to_string();
    assert!(
        !reply.contains("\"hi\""),
        "the handler must not answer a message its pooled guard refused: {reply}",
    );
    assert!(
        reply.contains("refused by an unmarked WS check"),
        "the pool ran the guard's own check_ws_message: {reply}",
    );

    app.shutdown().await.expect("the transport stops cleanly");
}

/// Refuses every message with an internal denial whose text names the server's
/// own address.
#[injectable]
#[derive(Default)]
struct LeakyInternalGuard;

impl Layer for LeakyInternalGuard {}

#[async_trait]
impl Guard for LeakyInternalGuard {
    async fn check_ws_message(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &serde_json::Value,
    ) -> Result<(), Denial> {
        Err(Denial::internal("secret detail: redis at 10.0.3.7 refused"))
    }
}

impl nest_rs_guards::WsGuard for LeakyInternalGuard {}

/// Refuses every message as a rate limit lifting in seven seconds.
#[injectable]
#[derive(Default)]
struct SevenSecondsGuard;

impl Layer for SevenSecondsGuard {}

#[async_trait]
impl Guard for SevenSecondsGuard {
    async fn check_ws_message(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &serde_json::Value,
    ) -> Result<(), Denial> {
        Err(Denial::rate_limited(7, "too many messages"))
    }
}

impl nest_rs_guards::WsGuard for SevenSecondsGuard {}

#[gateway(path = "/refusing")]
struct RefusingGateway;

#[messages]
impl RefusingGateway {
    #[subscribe_message("internal")]
    #[use_guards(LeakyInternalGuard)]
    #[public]
    async fn internal(&self) -> String {
        "unreachable".into()
    }

    #[subscribe_message("limited")]
    #[use_guards(SevenSecondsGuard)]
    #[public]
    async fn limited(&self) -> String {
        "unreachable".into()
    }
}

#[module(imports = [WsModule], providers = [RefusingGateway, LeakyInternalGuard, SevenSecondsGuard])]
struct RefusingModule;

/// A guard's internal denial answers the opaque message, never its text, and is
/// filed once, by the bridge that still holds the `Denial`, as the other three
/// edges file theirs.
#[tokio::test]
async fn a_per_message_internal_denial_answers_opaquely_and_is_filed_once_on_the_layers_target() {
    let logs = LogCapture::install();
    let app = nest_rs_testing::TestApp::builder()
        .module::<RefusingModule>()
        .build_ws()
        .await
        .expect("a gateway with per-message guards boots");

    let mut socket = app.socket("/refusing").connect().await;
    socket.send("internal", serde_json::Value::Null).await;
    let reply = socket.next_envelope().await;
    assert_eq!(reply["event"], "internal");
    assert_eq!(
        reply["data"]["error"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{reply}"
    );
    assert!(
        !reply.to_string().contains("secret detail"),
        "the guard's text reached the client: {reply}",
    );
    app.shutdown().await.expect("the transport stops cleanly");

    let denied = logs.expect_one(nest_rs_core::target::LAYERS, "guard denied the message");
    assert_eq!(denied.level, "warn");
    assert!(
        denied
            .field("guard")
            .is_some_and(|guard| guard.contains("LeakyInternalGuard")),
        "{denied:#?}",
    );
    assert_eq!(
        denied.field("event").as_deref(),
        Some("internal"),
        "{denied:#?}"
    );
    assert_eq!(
        denied.field("status").as_deref(),
        Some("500"),
        "{denied:#?}"
    );
    assert!(
        !format!("{denied:?}").contains("secret detail"),
        "the site files what it knows, never the guard's text: {denied:#?}",
    );
    assert!(
        logs.events()
            .iter()
            .all(|event| event.target != nest_rs_ws::TARGET || event.level != "warn"),
        "the denial is filed once, not again on the gateway's target: {:#?}",
        logs.events(),
    );
}

/// A rate limit's wait reaches the client in the frame's structured half.
#[tokio::test]
async fn a_per_message_rate_limit_carries_its_reason_and_its_wait() {
    let app = nest_rs_testing::TestApp::builder()
        .module::<RefusingModule>()
        .build_ws()
        .await
        .expect("a gateway with per-message guards boots");

    let mut socket = app.socket("/refusing").connect().await;
    socket.send("limited", serde_json::Value::Null).await;
    let reply = socket.next_envelope().await;
    assert_eq!(reply["data"]["error"], "too many messages", "{reply}");
    assert_eq!(reply["data"]["errors"]["reason"], "rate_limited", "{reply}");
    assert_eq!(reply["data"]["errors"]["code"], "RATE_LIMITED", "{reply}");
    assert_eq!(reply["data"]["errors"]["retryAfterSeconds"], 7, "{reply}");
    app.shutdown().await.expect("the transport stops cleanly");
}
