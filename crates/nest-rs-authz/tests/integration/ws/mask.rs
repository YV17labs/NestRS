//! Automatic reply masking through the real macro: `#[authorize(Action, Entity)]`
//! beside a `#[subscribe_message]` makes `#[messages]` emit `masked_reply_for`
//! around the reply — the gateway body writes no masking call.
//!
//! That last clause is the whole point of this file. Before it, a WS handler
//! returning entity rows had to hand-write `serde_json::to_value` +
//! `masked_reply` + two `map_err`s, which meant the posture was an `Action::Read`
//! argument buried in a function call rather than a greppable `#[authorize]` —
//! and a handler that simply forgot the call shipped unmasked rows and compiled.

use nest_rs_authz::Read;
use nest_rs_core::input;
// `WsError` is the handler-error type here purely because it is `Display` and
// already in scope — the suite is about the mask, not about error mapping.
use nest_rs_ws::{Gateway, WsClient, WsError, gateway, messages};

use super::{ability_for, body, dispatch_with, widget};

/// The wire shape: `name` optional so a field-restricted mask yields `null`
/// rather than an irreconcilable value.
#[input]
struct WidgetDto {
    id: i32,
    name: Option<String>,
}

/// A wire shape whose `name` is **required**. On GraphQL and MCP a mask that
/// strips it refuses the operation; on WS the frame just omits the key, which is
/// the difference `a_stripped_required_field_is_omitted_from_the_frame_not_refused`
/// pins.
#[input]
struct StrictWidgetDto {
    id: i32,
    name: String,
}

#[gateway(path = "/ws/mask")]
#[derive(Default)]
struct MaskGateway;

#[messages]
impl MaskGateway {
    /// Two widgets — the shape a real message replies with.
    #[subscribe_message("widgets")]
    #[authorize(Read, widget::Entity)]
    async fn widgets(&self) -> Result<Vec<WidgetDto>, WsError> {
        Ok(vec![
            WidgetDto {
                id: 1,
                name: Some("first".to_owned()),
            },
            WidgetDto {
                id: 2,
                name: Some("second".to_owned()),
            },
        ])
    }

    /// A scalar answer. The mask runs over it and finds nothing entity-shaped, so
    /// arming the posture on a count is harmless rather than an error.
    #[subscribe_message("widget_count")]
    #[authorize(Read, widget::Entity)]
    async fn widget_count(&self) -> Result<usize, WsError> {
        Ok(2)
    }

    /// The same row through a shape whose `name` is required.
    #[subscribe_message("strict_widget")]
    #[authorize(Read, widget::Entity)]
    async fn strict_widget(&self) -> Result<StrictWidgetDto, WsError> {
        Ok(StrictWidgetDto {
            id: 1,
            name: "first".to_owned(),
        })
    }

    /// `unmasked` keeps the gate and hands masking back to the body.
    #[subscribe_message("projected")]
    #[authorize(Read, widget::Entity, unmasked)]
    async fn projected(&self) -> Result<StrictWidgetDto, WsError> {
        Ok(StrictWidgetDto {
            id: 1,
            name: "first".to_owned(),
        })
    }

    /// The opt-out: no gate, no mask.
    #[subscribe_message("raw_widget")]
    #[public]
    async fn raw_widget(&self) -> Result<WidgetDto, WsError> {
        Ok(WidgetDto {
            id: 1,
            name: Some("first".to_owned()),
        })
    }
}

async fn reply(role: &str, event: &str) -> String {
    body(dispatch_with(&MaskGateway, ability_for(role), event).await)
}

#[tokio::test]
async fn an_unrestricted_caller_sees_every_field() {
    let body = reply("admin", "widgets").await;
    assert!(
        body.contains("first") && body.contains("second"),
        "nothing is masked for a caller whose grant restricts nothing: {body}",
    );
}

#[tokio::test]
async fn a_field_grant_strips_the_column_it_withholds() {
    let body = reply("viewer", "widgets").await;
    assert!(
        !body.contains("first") && !body.contains("second"),
        "a grant limited to `id` masks `name` out of every row, with no masking \
         call in the gateway body: {body}",
    );
    assert!(
        body.contains(r#""id":1"#),
        "…while the granted column still ships: {body}",
    );
}

#[tokio::test]
async fn a_row_the_ability_refuses_is_dropped() {
    let body = reply("auditor", "widgets").await;
    assert!(
        body.contains(r#""id":1"#),
        "the row the rule admits still ships: {body}",
    );
    assert!(
        !body.contains(r#""id":2"#),
        "…and a rule predicated on `id = 1` drops the other before it reaches a \
         model: {body}",
    );
}

#[tokio::test]
async fn a_scalar_answer_passes_through_the_mask() {
    let body = reply("viewer", "widget_count").await;
    assert!(
        body.contains('2'),
        "a `usize` answer has nothing entity-shaped to strip, so the posture still \
         gates but the mask is a no-op: {body}",
    );
}

// The case that separates WS from MCP. A stripped key the *return type* declares
// required is not a refusal here: the envelope promises no schema, so the frame
// simply omits it — HTTP's behaviour, not GraphQL's or MCP's. The masked JSON is
// what ships, so nothing ever has to fit back into `StrictWidgetDto`.
#[tokio::test]
async fn a_stripped_required_field_is_omitted_from_the_frame_not_refused() {
    let body = reply("viewer", "strict_widget").await;
    assert!(
        !body.starts_with("error:"),
        "a WS reply carries JSON, not a schema-checked value — the mask has nothing \
         to fail against: {body}",
    );
    assert!(
        !body.contains("first"),
        "…and the withheld column is gone from the frame: {body}",
    );
    assert!(
        body.contains(r#""id":1"#),
        "…while the granted one still ships: {body}",
    );
}

// What *is* fail-closed: nothing installed an ability, so nothing decided what
// this caller may read. Shipping the value would answer that with "everything".
#[tokio::test]
async fn no_ambient_ability_refuses_rather_than_shipping_the_rows() {
    let reply = MaskGateway
        .dispatch(&WsClient::for_test(), "widgets", serde_json::Value::Null)
        .await;
    let body = body(reply);
    assert!(
        body.starts_with("error:"),
        "a missing ability is a wiring failure, never a pass: {body}",
    );
    assert!(!body.contains("first"), "{body}");
}

#[tokio::test]
async fn unmasked_keeps_the_gate_and_returns_the_body_verbatim() {
    let body = reply("viewer", "projected").await;
    assert!(
        body.contains("first"),
        "`unmasked` is the opt-out for a shape the value-level round-trip cannot \
         see through — the gate still ran, the mask did not: {body}",
    );
}

#[tokio::test]
async fn public_opts_out_of_masking() {
    let body = reply("viewer", "raw_widget").await;
    assert!(
        body.contains("first"),
        "`#[public]` declares no gate and no mask — the message is responsible for \
         its own output: {body}",
    );
}
