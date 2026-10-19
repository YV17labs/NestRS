//! The class-level gate `#[authorize(Action, Entity)]` desugars to on a
//! message, run in the message's guard stage over a real upgrade, and ahead
//! of the handler when `Gateway::dispatch` calls it on its own.
//!
//! Includes the WS-specific case: a gateway whose upgrade attached no ability
//! has none ambient when a message arrives, and fails closed.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_authz::{Read, Update};
use nest_rs_core::module;
use nest_rs_testing::TestApp;
use nest_rs_ws::{Gateway, WsClient, WsError, WsModule, WsReply, gateway, messages};

use crate::widget;
use crate::ws::{RoleUpgrade, ability_for, body, dispatch_with, message_as};

/// How many times `touch`'s body ran.
static TOUCHED: AtomicUsize = AtomicUsize::new(0);

#[gateway(path = "/ws/gate")]
#[use_guards(RoleUpgrade)]
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

    #[subscribe_message("touch")]
    #[authorize(Update, widget::Entity)]
    async fn touch(&self) -> Result<(), WsError> {
        TOUCHED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(imports = [WsModule], providers = [RoleUpgrade, GateGateway])]
struct GateModule;

async fn app() -> nest_rs_testing::ws::WsApp {
    TestApp::builder()
        .module::<GateModule>()
        .build_ws()
        .await
        .expect("the gateway boots")
}

/// What `role` reads sending `event`: the reply, or the error frame's message.
async fn answered(role: Option<&str>, event: &str) -> String {
    let app = app().await;
    let frame = message_as(&app, "/ws/gate", role, event).await;
    app.shutdown().await.expect("the transport stops cleanly");
    match frame["data"].get("error") {
        Some(error) => format!("error: {error}"),
        None => frame["data"].to_string(),
    }
}

#[tokio::test]
async fn a_granted_action_reaches_the_handler() {
    let body = answered(Some("admin"), "read").await;
    assert!(body.contains("served"), "{body}");
}

#[tokio::test]
async fn an_action_the_ability_does_not_grant_is_refused() {
    let body = answered(Some("admin"), "write").await;
    assert!(
        !body.contains("written"),
        "a `Read` grant does not carry `Update` — the gate refuses before the \
         handler body: {body}",
    );
    assert!(body.starts_with("error:"), "{body}");
}

#[tokio::test]
async fn an_ability_that_grants_nothing_is_refused() {
    let body = answered(Some("nobody"), "read").await;
    assert!(
        !body.contains("served"),
        "an ability with no rule for this subject refuses: {body}",
    );
}

/// An upgrade no ability guard ran on: nothing installed an ability.
#[tokio::test]
async fn no_ambient_ability_fails_closed() {
    let body = answered(None, "read").await;
    assert!(
        body.starts_with("error:") && !body.contains("served"),
        "a missing ability is a wiring failure, never a pass: {body}",
    );
}

#[tokio::test]
async fn a_public_message_needs_no_ability_at_all() {
    let body = answered(None, "open").await;
    assert!(
        body.contains("open"),
        "`#[public]` emits no gate, so it does not depend on an ambient ability: {body}",
    );
}

/// A rule the credential's scopes withhold is refused like an ungranted one,
/// and the refusal names the scope.
#[tokio::test]
async fn a_scoped_rule_the_credential_does_not_carry_is_refused() {
    let body = answered(Some("scoped"), "read").await;
    assert!(
        !body.contains("served"),
        "a credential that delegated no scope withholds the rule, so the gate \
         refuses exactly as for a rule nobody wrote: {body}",
    );
}

/// `Gateway::dispatch` runs the posture's class gate before the handler: a
/// write the ability does not grant answers the gate's refusal, and its body
/// never runs.
#[tokio::test]
async fn dispatch_refuses_an_ungranted_write_before_its_body() {
    let before = TOUCHED.load(Ordering::SeqCst);
    let WsReply::Error(refused) = dispatch_with(&GateGateway, ability_for("admin"), "touch").await
    else {
        panic!("a `Read` grant does not carry `Update`");
    };
    assert_eq!(
        refused.errors.as_ref().map(|errors| &errors["code"]),
        Some(&serde_json::json!("FORBIDDEN")),
        "the refusal reads as the socket's: {refused:?}",
    );
    assert_eq!(TOUCHED.load(Ordering::SeqCst), before, "the body never ran");
}

/// With no ambient ability, `dispatch` fails closed rather than running the
/// handler.
#[tokio::test]
async fn dispatch_without_an_ambient_ability_fails_closed() {
    let before = TOUCHED.load(Ordering::SeqCst);
    let reply = GateGateway
        .dispatch(&WsClient::for_test(), "touch", serde_json::Value::Null)
        .await;
    let body = body(reply);
    assert!(body.starts_with("error:"), "{body}");
    assert_eq!(TOUCHED.load(Ordering::SeqCst), before, "the body never ran");
}

#[tokio::test]
async fn dispatch_serves_a_granted_action_and_a_public_message() {
    let read = body(dispatch_with(&GateGateway, ability_for("admin"), "read").await);
    assert!(read.contains("served"), "{read}");
    let open = body(
        GateGateway
            .dispatch(&WsClient::for_test(), "open", serde_json::Value::Null)
            .await,
    );
    assert!(
        open.contains("open"),
        "`#[public]` has no gate to run: {open}"
    );
}
