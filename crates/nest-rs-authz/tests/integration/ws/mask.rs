//! Automatic reply masking through the real macro: `#[authorize(Action, Entity)]`
//! beside a `#[subscribe_message]` makes `#[messages]` emit the mask
//! around the reply — the gateway body writes no masking call.

use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, Action, Read, current_ability};
use nest_rs_core::input;
// `WsError` is the handler error only because it is `Display` and in scope.
use nest_rs_ws::{Gateway, WsClient, WsError, gateway, messages};

use super::{ability_for, body, dispatch_with, widget};
use crate::post;

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

/// A post as a message replies with it.
#[input]
struct PostDto {
    id: i32,
    org_id: i32,
    title: String,
}

/// A wire shape whose `id` no widget's column can read back.
#[input]
struct InventedWidgetDto {
    id: String,
}

/// The alias a macro reading the return's spelling cannot see through.
type Replied<T> = Result<T, WsError>;

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

    /// The rows a `Repo` read under the caller's ability returns: posts of
    /// orgs 7 and 8.
    #[subscribe_message("posts")]
    #[authorize(Read, post::Entity)]
    async fn posts(&self) -> Result<Vec<PostDto>, WsError> {
        Ok([(1, 7), (2, 8)]
            .into_iter()
            .map(|(id, org_id)| PostDto {
                id,
                org_id,
                title: format!("post {id}"),
            })
            .collect())
    }

    /// A value with no `Result` around it masks as one inside it does.
    #[subscribe_message("bare_widgets")]
    #[authorize(Read, widget::Entity)]
    async fn bare_widgets(&self) -> Vec<WidgetDto> {
        vec![
            WidgetDto {
                id: 1,
                name: Some("first".to_owned()),
            },
            WidgetDto {
                id: 2,
                name: Some("second".to_owned()),
            },
        ]
    }

    /// A `Result` behind an alias masks by its type, never as `{"Ok": …}`.
    #[subscribe_message("aliased_widgets")]
    #[authorize(Read, widget::Entity)]
    async fn aliased_widgets(&self) -> Replied<Vec<WidgetDto>> {
        Ok(vec![WidgetDto {
            id: 1,
            name: Some("first".to_owned()),
        }])
    }

    /// The alias's failure answers as any handler's does.
    #[subscribe_message("aliased_failure")]
    #[authorize(Read, widget::Entity)]
    async fn aliased_failure(&self) -> Replied<Vec<WidgetDto>> {
        Err(WsError::new("refused by the body"))
    }

    #[subscribe_message("widgets_or_none")]
    #[authorize(Read, widget::Entity)]
    async fn widgets_or_none(&self) -> Result<Vec<Option<WidgetDto>>, WsError> {
        Ok(vec![
            Some(WidgetDto {
                id: 1,
                name: Some("first".to_owned()),
            }),
            None,
        ])
    }

    /// Nothing to reply: the gate alone holds.
    #[subscribe_message("nothing")]
    #[authorize(Read, widget::Entity)]
    async fn nothing(&self) {}

    /// A value the mask cannot read back into a row.
    #[subscribe_message("invented_widget")]
    #[authorize(Read, widget::Entity)]
    async fn invented_widget(&self) -> Result<InventedWidgetDto, WsError> {
        Ok(InventedWidgetDto {
            id: "anonymous".to_owned(),
        })
    }

    /// No posture: no gate, no mask.
    #[subscribe_message("raw_widget")]
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

/// The posture means one thing whatever the return type spells.
#[tokio::test]
async fn a_bare_value_and_an_aliased_result_are_masked_as_a_literal_result_is() {
    for event in ["bare_widgets", "aliased_widgets"] {
        let body = reply("viewer", event).await;
        assert!(body.contains(r#""id":1"#), "{event}: {body}");
        assert!(!body.contains("first"), "{event}: {body}");
        assert!(!body.contains("Ok"), "{event}: {body}");
    }
}

#[tokio::test]
async fn an_aliased_results_failure_is_an_error_frame_never_a_masked_reply() {
    let body = reply("admin", "aliased_failure").await;
    assert!(
        body.starts_with("error:") && body.contains("refused by the body"),
        "{body}"
    );
}

#[tokio::test]
async fn a_message_replying_nothing_keeps_its_gate() {
    assert_eq!(reply("admin", "nothing").await, "none");
    let refused = reply("nobody", "nothing").await;
    assert!(refused.starts_with("error:"), "{refused}");
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

// A stripped key the return type requires is not a refusal on WS: the frame
// omits it, as an HTTP body does.
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

/// A `null` holds no row: it stays beside what the mask cut.
#[tokio::test]
async fn a_null_item_of_a_list_stays_null() {
    let body = reply("viewer", "widgets_or_none").await;
    assert_eq!(body, r#"[{"id":1},null]"#);
}

// What is fail-closed: no ambient ability installed.
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

/// A failure the operator reads names the message it answered, as its `mounted`
/// line does.
#[tokio::test]
async fn a_value_the_mask_cannot_read_back_names_its_message() {
    let logs = nest_rs_testing::LogCapture::install();
    let body = reply("viewer", "invented_widget").await;
    assert!(body.starts_with("error:"), "{body}");
    assert!(!body.contains("anonymous"), "{body}");
    let event = logs.expect_one("nest_rs::authz", "response masking failed");
    for (field, named) in [
        ("transport", "ws"),
        ("host", "MaskGateway"),
        ("method", "invented_widget"),
        ("event", "invented_widget"),
    ] {
        assert_eq!(event.field(field).as_deref(), Some(named), "{field}");
    }
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
async fn a_message_stating_no_posture_arms_no_mask() {
    let body = reply("viewer", "raw_widget").await;
    assert!(
        body.contains("first"),
        "no gate and no mask: the message is responsible for its own output: {body}",
    );
}

/// The read behind the reply already left out what a relational denial names,
/// so the mask keeps every row it was handed.
#[tokio::test]
async fn a_relational_denial_keeps_every_row_the_read_admitted() {
    let mut member = AbilityBuilder::new();
    member.can(Action::Read, post::Entity);
    member
        .cannot(Action::Read, post::Entity)
        .when(|p| post::in_org(&p, 9));
    let member = Arc::new(member.build().expect("valid test ability"));

    let body = body(dispatch_with(&MaskGateway, member, "posts").await);

    assert!(
        body.contains(r#""id":1"#) && body.contains(r#""id":2"#),
        "{body}"
    );
}

/// What each connection hook of [`HookGateway`] read, by the ability it held.
#[nest_rs_core::injectable]
#[derive(Default)]
struct HookReads {
    connect: std::sync::OnceLock<bool>,
    disconnect: std::sync::OnceLock<bool>,
}

#[gateway(path = "/ws/hooks")]
#[use_guards(super::RoleUpgrade)]
struct HookGateway {
    #[inject]
    reads: Arc<HookReads>,
}

#[messages]
impl HookGateway {
    #[on_connect]
    async fn joined(&self) {
        let _ = self.reads.connect.set(current_ability().is_some());
    }

    #[on_disconnect]
    async fn left(&self) {
        let _ = self.reads.disconnect.set(current_ability().is_some());
    }

    #[subscribe_message("reads")]
    #[authorize(Read, widget::Entity)]
    async fn reads(&self) -> Result<bool, WsError> {
        Ok(current_ability().is_some())
    }
}

#[nest_rs_core::module(
    imports = [nest_rs_ws::WsModule],
    providers = [super::RoleUpgrade, HookReads, HookGateway],
)]
struct HookModule;

/// A hook answers no reply a mask could shape, so what it reads ships as it stands:
/// the ability the upgrade attached is withheld from it, as from any such unit, and
/// the socket's messages keep it.
#[tokio::test]
async fn a_connection_hook_reads_nothing_a_grant_guards() {
    let app = nest_rs_testing::TestApp::builder()
        .module::<HookModule>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let reads = app
        .container()
        .get::<HookReads>()
        .expect("a provider of the app");

    let frame = super::message_as(&app, "/ws/hooks", Some("admin"), "reads").await;
    assert_eq!(frame["data"], true, "a message keeps it: {frame}");

    nest_rs_testing::wait_until(std::time::Duration::from_secs(5), || {
        reads.disconnect.get().is_some()
    })
    .await;
    assert_eq!(reads.connect.get(), Some(&false), "the connect hook read");
    assert_eq!(
        reads.disconnect.get(),
        Some(&false),
        "the disconnect hook read"
    );
    app.shutdown().await.expect("the transport stops cleanly");
}
