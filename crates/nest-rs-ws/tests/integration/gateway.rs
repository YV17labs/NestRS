//! `#[messages]`-generated `Gateway::dispatch` — the return-type shape paths
//! the macro picks (Unit / Value / `Result<(), E>` / `Result<T, E>`) — and the
//! address the generated `Discoverable` mounts that dispatcher at. The macro
//! itself lives in `nest-rs-ws-macros`; this file pins its observable behaviour.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_pipes::{ParseArray, Pipe, PipeError, Piped, Trim, Valid};
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

/// The same shape with an error that is `Display` and **not** `Error` — the one
/// a bound on the alias path let fall to the blanket impl and ship whole.
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

/// What every `decode_*` handler decodes: a number, sent a secret.
const SECRET_BODY: &str = r#""sk_live_51HsecretTOKEN""#;

/// A `Display`-only error spelling a decode failure — the third tier, which has
/// no chain to read it off.
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

/// An alias whose error is `Display` alone is still a `Result`: it replies with
/// an error frame and a `warn`, never with the error struct serialized as data.
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
        WsReply::Error(msg) => assert_eq!(msg.error, "display-only failure"),
        WsReply::Reply(value) => panic!("the Err variant was shipped as a success frame — {value}"),
        WsReply::None => panic!("expected an error frame"),
    }
    logs.expect_one("nest_rs::ws", "subscribe_message handler returned Err");
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

/// A handler returns at most a `Result` around a `Result`, and the inner `Err` is
/// a failure like the outer one. It used to be the `Ok` value, serialized whole:
/// `{"Err": {"dsn": …}}` in a frame shaped like a success, with no `warn`.
#[tokio::test]
async fn a_result_inside_a_result_is_an_error_frame_however_it_is_spelled() {
    for event in ["nested_literal", "nested_renamed"] {
        let logs = nest_rs_testing::LogCapture::install();
        match TestGateway
            .dispatch(&WsClient::for_test(), event, serde_json::Value::Null)
            .await
        {
            WsReply::Error(msg) => assert_eq!(msg.error, "database unavailable", "{event}"),
            WsReply::Reply(value) => {
                panic!("`{event}`: the inner Err was shipped as a success frame — {value}")
            }
            WsReply::None => panic!("`{event}`: expected an error frame"),
        }
        let line = logs.expect_one("nest_rs::ws", "subscribe_message handler returned Err");
        assert_eq!(line.field("event").as_deref(), Some(event));
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

/// An error that is not `Send + Sync` cannot become a boxed `Send + Sync` one,
/// and it used to fall to the `Display` fallback — the operator's line carried
/// its sentence and lost every cause beneath it. It is still an `Error`, so its
/// chain is walked.
#[tokio::test]
async fn an_error_that_is_not_send_logs_its_cause_chain() {
    for event in ["unsendable_boxed", "unsendable_rc"] {
        let logs = nest_rs_testing::LogCapture::install();
        match TestGateway
            .dispatch(&WsClient::for_test(), event, serde_json::Value::Null)
            .await
        {
            WsReply::Error(msg) => assert_eq!(msg.error, "read failed", "{event}"),
            _ => panic!("`{event}`: expected an error frame"),
        }
        let line = logs.expect_one("nest_rs::ws", "subscribe_message handler returned Err");
        assert_eq!(
            line.field("error").as_deref(),
            Some("read failed: replica lagging"),
            "`{event}` logs the whole chain",
        );
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

/// A payload that does not decode is reported by where and what kind, never by
/// the value — in the line and in the frame. serde's sentence quoted the value,
/// so whatever a client put in the wrong field reached the server's log.
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

/// A list item a pipe refuses is said without its value, in the frame and on the
/// line. `ParseArray`'s refusal quoted the item, and the frame and the `warn`
/// both carry a pipe's refusal whole.
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

/// A validation failure a handler returns is said without the value it refused,
/// in the frame and on the line, whether its error keeps the failure as its
/// source or spells it. The line renders the error's whole chain and the frame
/// the error's own sentence, and validator's own `Display` prints every rule's
/// parameters, the submitted value among them.
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
        logs.expect_one(nest_rs_ws::TARGET, "subscribe_message handler returned Err");
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

// ── The mount address: `#[gateway(version = …)]` ────────────────────────────
//
// A gateway's mount is a path a client selects, so it declares a version the
// way a controller does and resolves it through the same
// `nest_rs_http::version_path`. What follows pins the three things that
// declaration decides: the effective path, what the router serves, and what the
// boot does with two gateways that share a path.

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
/// is right — refusing it with no reply and no log at any level is a silent
/// failure, and from the client's side it is indistinguishable from a handler
/// that never answered.
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

// ── Connection hooks under `#[cfg]` ─────────────────────────────────────────
//
// An attribute macro reads a method before its `#[cfg]` is evaluated, so every
// hook reaches the expansion — including one compiled out. Keeping only the
// last one seen dropped a hook that *is* compiled in whenever a compiled-out
// one followed it.

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

/// A decode failure a handler returns reaches neither its frame nor the
/// operator's line with the value, in every tier a handler's error takes —
/// `anyhow::Result` with `?`, serde's error itself, an error that is not
/// `Send`, and a `Display`-only one. The frame carried the error's sentence,
/// which is serde's, which quotes it; and a value a handler failed to decode
/// need not even be this client's.
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
            format!("could not read the amount: {REPORT}"),
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

/// A [`WsError`](nest_rs_ws::WsError) a handler returns through anyhow is the
/// handler's deliberate frame, and is sent whole: anyhow's own box hid it from
/// the downcast, and the per-field detail was lost.
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

// ── The way down ────────────────────────────────────────────────────────────
//
// A socket is a connection poem stops tracking at the upgrade, so the shutdown
// window neither waited for one nor closed it: the transport returned with
// every socket still open, the shutdown hooks ran under live handlers, and the
// process exit cut them — 1006 to a client that could not tell a deploy from a
// network fault. At the signal a socket is now told RFC 6455 §7.4.1's **1001
// Going Away**, the code for "a server going down", after whatever message it
// is already answering.

static SLOW_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();
static STUCK_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

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

/// An idle socket is closed at the signal with 1001 and a reason saying what
/// to do — reconnect — and the transport waits for that close rather than
/// returning past a socket it never told.
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

/// A message the socket is already answering at the signal is answered — inside
/// the window, like a request still running — and the close comes after it, so
/// no reply is lost to the shutdown.
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

/// A message still running when the window closes is dropped where it waits,
/// and files its line `cancelled` — the socket goes with it, cut like a request
/// the window closed on.
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

/// A handler that panics took the connection task down with it: no line, and a
/// socket the peer read as 1006. The panic is contained at the message — the
/// unit files `panic`, the client is answered with an error frame that says
/// nothing of what unwound, and the socket goes on serving.
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
    // The message's span fails with the line's word.
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

/// A connect hook that panics leaves a connection set up halfway, so it is not
/// served: the hook files `panic` and the socket is closed with §7.4.1's **1011
/// Internal Error**, which a client reads as "the server failed, try again" —
/// rather than the 1006 the unwinding task left it.
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

/// A disconnect hook that panics is the last thing a socket does, so the close
/// it was running for still completes — the §5.5.1 echo reaches the client —
/// and the hook files `panic`.
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

/// The writer that drains a socket's replies owns its sink, and ran on a task
/// of its own that nothing stopped: a client that stopped reading parked it,
/// and the socket stayed open after the transport had stopped the connection —
/// past everything that served it. It goes with its connection now, so the
/// window's close cuts the socket like any other.
#[tokio::test]
async fn a_socket_whose_peer_stopped_reading_goes_with_its_connection() {
    let logs = LogCapture::install();
    let app = leaving_app().await;
    let mut socket = app.socket("/leaving").connect().await;
    socket.send("flood", serde_json::Value::Null).await;
    // Never read again: the replies fill both kernel buffers and park the
    // writer. On paused time, which moves only once it is parked, through the
    // window too.
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

/// Refuses every message, and declares no `WsGuard`: the marker is a bound
/// `#[messages]` emits for the guards declared at its site, and the global pool
/// — `Arc<dyn Guard>`, with no marker to consult — is not one of them.
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

/// A pooled guard checks every message whether or not it declares `WsGuard`, so
/// a missing marker on a global guard costs a compile-time bound and nothing on
/// the wire — the other three edges are held in `nest-rs-testing`'s
/// `guard_markers`.
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
