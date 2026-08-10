//! The class-level gate `#[authorize(Action, Entity)]` desugars to on a message.
//!
//! Same three refusals every transport's gate answers with, and one WS-specific
//! case that matters more here than anywhere else: a gateway whose data context
//! was never registered has **no** ambient ability by the time a message arrives
//! — the upgrade's task-locals unwound when it returned — so "no ability" must
//! fail closed rather than read as unrestricted.

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

// The case a gateway hits when `AuthzWsModule` was never imported: nothing
// installed an ability, and the message must not be served on the strength of
// that absence.
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

// A rule the credential's scopes withhold is a rule nobody wrote: the gate
// refuses exactly as it does for an ungranted action, and the refusal names the
// scope so a client can act on it.
#[tokio::test]
async fn a_scoped_rule_the_credential_does_not_carry_is_refused() {
    // `Some([])` is an OAuth credential that delegated *nothing* — distinct from
    // the default `None`, which means "not scope-aware" and applies scoped rules
    // in full. Conflating the two is the fail-open reading.
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
