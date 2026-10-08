//! The class-level gate `#[authorize(Action, Entity)]` desugars to on a message.
//!
//! Includes the WS-specific case: a gateway whose data context was never
//! registered has no ambient ability when a message arrives, and fails closed.

use nest_rs_authz::{AbilityBuilder, Action, Read, Update};
use nest_rs_ws::{Gateway, WsClient, WsError, WsReply, gateway, messages};
use std::sync::Arc;

use super::{ability_for, body, dispatch_with, widget};

#[gateway(path = "/ws/gate")]
#[derive(Default)]
struct GateGateway;

#[messages]
impl GateGateway {
    #[subscribe_message("read")]
    #[authorize(Read, widget::Entity)]
    async fn read(&self) -> Result<String, WsError> {
        Ok("served".to_owned())
    }

    #[subscribe_message("write")]
    #[authorize(Update, widget::Entity)]
    async fn write(&self) -> Result<String, WsError> {
        Ok("written".to_owned())
    }

    #[subscribe_message("open")]
    #[public]
    async fn open(&self) -> Result<String, WsError> {
        Ok("open".to_owned())
    }
}

#[tokio::test]
async fn a_granted_action_reaches_the_handler() {
    let body = body(dispatch_with(&GateGateway, ability_for("admin"), "read").await);
    assert!(body.contains("served"), "{body}");
}

#[tokio::test]
async fn an_action_the_ability_does_not_grant_is_refused() {
    let body = body(dispatch_with(&GateGateway, ability_for("admin"), "write").await);
    assert!(
        !body.contains("written"),
        "a `Read` grant does not carry `Update` — the gate refuses before the \
         handler body: {body}",
    );
    assert!(body.starts_with("error:"), "{body}");
}

#[tokio::test]
async fn an_ability_that_grants_nothing_is_refused() {
    let body = body(dispatch_with(&GateGateway, ability_for("nobody"), "read").await);
    assert!(
        !body.contains("served"),
        "an ability with no rule for this subject refuses: {body}",
    );
}

// A gateway that never imported `AuthzWsModule`: nothing installed an ability.
#[tokio::test]
async fn no_ambient_ability_fails_closed() {
    let reply = GateGateway
        .dispatch(&WsClient::for_test(), "read", serde_json::Value::Null)
        .await;
    match reply {
        WsReply::Error(error) => assert!(
            !error.error.contains("served"),
            "a missing ability is a wiring failure, never a pass: {error}",
        ),
        other => panic!("expected a refusal, got {}", body(other)),
    }
}

#[tokio::test]
async fn a_public_message_needs_no_ability_at_all() {
    let reply = GateGateway
        .dispatch(&WsClient::for_test(), "open", serde_json::Value::Null)
        .await;
    assert!(
        body(reply).contains("open"),
        "`#[public]` emits no gate, so it does not depend on the data context",
    );
}

// A rule the credential's scopes withhold is refused like an ungranted one, and
// the refusal names the scope.
#[tokio::test]
async fn a_scoped_rule_the_credential_does_not_carry_is_refused() {
    // `Some([])` delegated nothing; the default `None` means "not scope-aware".
    // Conflating the two is the fail-open reading.
    let mut builder = AbilityBuilder::new().with_granted_scopes(Some(Arc::from([])));
    builder
        .can(Action::Read, widget::Entity)
        .requires_scope("widgets:read");
    let ability = Arc::new(builder.build().expect("valid test ability"));
    let body = body(dispatch_with(&GateGateway, ability, "read").await);
    assert!(
        !body.contains("served"),
        "a credential that delegated no scope withholds the rule, so the gate \
         refuses exactly as for a rule nobody wrote: {body}",
    );
}
