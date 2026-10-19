//! A gateway's guards: the upgrade's, run on the HTTP `GET` that opens a
//! socket, and each message's guard stage — the app-wide pool and the
//! message's own `#[use_guards]`, composed through the guards crate's
//! site-chain composition, then the posture's class gate — run by the
//! pipeline as its site's guard stage.

use std::any::TypeId;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use nest_rs_core::__private::{
    ClassGate, GuardStage, ResolvedLayer, new_unit_context, report_unresolved,
};
use nest_rs_core::{Container, Failure, Handler, Peer, UnitContext};
use nest_rs_guards::__private::{
    GlobalBucket, Refused, SiteChainSources, boot_validate_guards, check_guards, client_message,
    compose_site_chain, denial_of, failure_chain, structured_reason,
};
use nest_rs_guards::{Denial, Guard, GuardSpec};
use nest_rs_http::__private::OriginRefused;

use crate::envelope::{WsError, WsReply};
use crate::message::message_site;
use crate::server::WsClient;
use crate::view::WsView;

/// What a gateway's upgrade chain is named in boot logs and refusals.
fn upgrade_site(path: &str) -> String {
    format!("the {path} gateway upgrade")
}

/// What a message's guard stage composes, as `#[messages]` collects it.
pub struct MessageGuards {
    /// `#[use_guards]` beside the message, `#[force_guards]` included.
    pub method: Vec<GuardSpec>,
    /// `#[force_guards]`: the guards the message re-runs.
    pub force: Vec<TypeId>,
    /// The class gate the message's posture declares.
    pub gate: Option<ClassGate>,
}

/// The guard stage of the message `handler` declares: the app-wide pool and
/// the message's own guards, each once unless `#[force_guards]` replays one,
/// then its class gate — `None` when it runs none. The gateway's own guards
/// ran at the upgrade.
pub fn message_guard_stage(
    container: &Container,
    handler: &Handler,
    declared: MessageGuards,
) -> nest_rs_core::anyhow::Result<Option<Arc<dyn GuardStage>>> {
    let sources = SiteChainSources {
        provider: Vec::new(),
        method: declared.method,
        force: declared.force,
    };
    let guards = compose_site_chain(
        container,
        &message_site(handler),
        sources,
        GlobalBucket::Fold,
    )
    .inspect_err(report_unresolved)?;
    if guards.is_empty() && declared.gate.is_none() {
        return Ok(None);
    }
    Ok(Some(Arc::new(MessageGuardStage {
        guards,
        gate: declared.gate,
    })))
}

/// A message's guards, then its class gate, as the pipeline runs a site's
/// guard stage: a refusal is answered into the message's view, so no
/// exception filter maps it.
struct MessageGuardStage {
    guards: Vec<ResolvedLayer<dyn Guard>>,
    gate: Option<ClassGate>,
}

impl GuardStage for MessageGuardStage {
    fn check<'a>(&'a self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            match check_guards(&self.guards, self.gate, cx).await {
                Ok(()) => Ok(()),
                Err(refused) => Err(refuse(cx, refused)),
            }
        })
    }
}

/// The refused message's answer: the denial filed by the stage when its guard
/// filed none, rendered into the view, the unit failed with it, answered.
fn refuse(cx: &mut UnitContext<'_>, refused: Refused) -> Failure {
    let connection = cx.peer().and_then(|peer| peer.connection);
    let event = cx.handler().address();
    let denial = refused.into_denial(|guard, denial| {
        // Structural floor mirroring the HTTP edge's, filed where the denial
        // is still held; its text never reaches the line.
        tracing::warn!(
            target: nest_rs_core::target::LAYERS,
            guard,
            conn_id = connection,
            event,
            status = denial.http_status(),
            error = failure_chain(denial),
            "guard denied the message",
        );
    });
    let frame = denial_to_ws_error(denial.clone());
    match WsView::of(cx) {
        Ok(view) => Failure::from(denial).answered(view.respond(WsReply::Error(frame))),
        Err(viewless) => viewless,
    }
}

/// Render a guard's [`Denial`] as the error frame one message answers with.
///
/// No status line: the message, plus `code`, `reason` and `requiredScopes`
/// under the frame's `data.errors`, where a `Valid<T>` rejection's details
/// ride. An internal denial is opaque, and an unavailable dependency answers its
/// condition, never what the guard said of it.
pub fn denial_to_ws_error(denial: Denial) -> WsError {
    if matches!(denial, Denial::Internal(_)) {
        return WsError::new(nest_rs_core::OPAQUE_CLIENT_MESSAGE);
    }
    WsError::with_details(
        client_message(&denial),
        serde_json::Value::Object(structured_reason(&denial)),
    )
}

/// The posture's class `gate` the message `handler` declares, run alone on
/// `data`, arrived on `client` — no guard runs — as `Gateway::dispatch` runs
/// it: `data` back, or the frame its refusal answers, the gate filing its own.
pub(crate) fn gate_alone(
    client: &WsClient,
    handler: &'static Handler,
    gate: ClassGate,
    data: serde_json::Value,
) -> Result<serde_json::Value, WsError> {
    let mut view = WsView::new(client.clone(), handler.address(), data);
    let peer = Peer::new(None, Some(client.id()));
    let gated = gate(&mut new_unit_context(handler, &mut view, Some(peer), None));
    match gated {
        Ok(()) => Ok(view.take_data()),
        Err(refused) => Err(denial_to_ws_error(denial_of(&refused))),
    }
}

/// The upgrade's guards of the gateway at `path`, phase-validated (an HTTP
/// `GET`, the entry the check reads) and resolvable on `container`.
pub(crate) fn validate_upgrade(
    container: &Container,
    path: &str,
    upgrade: &[GuardSpec],
) -> nest_rs_core::anyhow::Result<()> {
    boot_validate_guards(container, upgrade, &upgrade_site(path))
        .map_err(nest_rs_core::anyhow::Error::msg)
}

/// A gateway's upgrade from a browser origin the transport's policy refuses,
/// filed on this edge's target.
pub fn refused_origin(refused: &OriginRefused) {
    nest_rs_http::__origin_refused!(target: crate::TARGET, refused);
}

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler};
    use nest_rs_core::layer_chain::LayerSite;
    use nest_rs_core::{AmbientValue, Layer, Posture};
    use nest_rs_guards::async_trait;
    use serde_json::json;

    use super::*;

    struct Gateway;

    static SEND: Handler = declare_handler(HandlerDeclaration {
        unit: crate::unit::MESSAGE,
        host: TypeId::of::<Gateway>(),
        host_name: "Gateway",
        method: "send",
        address: "send",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    #[derive(Debug, PartialEq)]
    struct Seen(&'static str, u64);

    impl AmbientValue for Seen {}

    /// Admits a message whose `data` is a number, attaching what it read.
    struct Numbers;

    impl Layer for Numbers {}

    #[async_trait]
    impl Guard for Numbers {
        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            let connection = cx.peer().and_then(|peer| peer.connection);
            let view = cx
                .switch_to::<WsView>()
                .ok_or_else(|| Denial::internal("a message has a WebSocket view"))?;
            let numeric = view.data().is_some_and(serde_json::Value::is_number);
            let seen = Seen(
                if view.event() == "send" {
                    "send"
                } else {
                    "other"
                },
                connection.unwrap_or_default(),
            );
            if !numeric {
                return Err(Denial::forbidden("a number, please"));
            }
            cx.attach(seen);
            Ok(())
        }
    }

    fn stage_of<G: Guard + 'static>(guard: G) -> MessageGuardStage {
        MessageGuardStage {
            guards: vec![ResolvedLayer {
                type_id: TypeId::of::<G>(),
                name: std::any::type_name::<G>(),
                source: LayerSite::Method,
                layer: Arc::new(guard),
            }],
            gate: None,
        }
    }

    /// Run `stage` on a message of `data`: what it came to, and the view.
    async fn checked(
        stage: &MessageGuardStage,
        client: &WsClient,
        data: serde_json::Value,
    ) -> (Result<(), Failure>, WsView, Option<std::sync::Arc<Seen>>) {
        let mut view = WsView::new(client.clone(), "send", data);
        let peer = nest_rs_core::Peer::new(None, Some(client.id()));
        let (checked, seen) = {
            let mut cx = new_unit_context(&SEND, &mut view, Some(peer), None);
            let checked = stage.check(&mut cx).await;
            let seen = cx.ambient::<Seen>();
            (checked, seen)
        };
        (checked, view, seen)
    }

    #[tokio::test]
    async fn a_guard_reads_the_message_and_its_connection_and_attaches_for_the_handler() {
        let client = WsClient::for_test();
        let (checked, view, seen) = checked(&stage_of(Numbers), &client, json!(7)).await;
        assert!(checked.is_ok());
        assert_eq!(
            view.data(),
            Some(&json!(7)),
            "the handler gets the data back"
        );
        assert_eq!(seen.as_deref(), Some(&Seen("send", client.id())));
    }

    #[tokio::test]
    async fn a_denial_answers_the_frame_and_is_filed() {
        let logs = nest_rs_testing::LogCapture::install();
        let (checked, view, _) =
            checked(&stage_of(Numbers), &WsClient::for_test(), json!("text")).await;
        let failure = checked.expect_err("text is refused");
        assert!(failure.is_answered(), "the stage answered it");
        let (reply, line) = view.into_answer();
        let Some(crate::WsReply::Error(refused)) = reply else {
            panic!("a refusal answers an error frame");
        };
        assert_eq!(refused.error, "a number, please");
        assert_eq!(
            refused.errors.as_ref().map(|e| &e["code"]),
            Some(&json!("FORBIDDEN"))
        );
        assert!(line.is_none(), "the stage filed the denial itself");
        let event = logs.expect_one("nest_rs::layers", "guard denied the message");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("event").as_deref(), Some("send"));
        assert_eq!(event.field("error"), None, "a refused caller is no failure");
    }

    /// What a store refusing the connection reports, the cause a guard keeps.
    #[derive(Debug)]
    struct ConnectionRefused;

    impl std::fmt::Display for ConnectionRefused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the identity store refused the connection")
        }
    }

    impl std::error::Error for ConnectionRefused {}

    /// Cannot evaluate a message: its identity store is down.
    struct StoreDown;

    impl Layer for StoreDown {}

    #[async_trait]
    impl Guard for StoreDown {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Err(Denial::unavailable_because(
                None,
                "the identity store did not answer",
                ConnectionRefused,
            ))
        }
    }

    #[tokio::test]
    async fn a_denial_keeping_its_cause_files_the_chain_and_answers_without_it() {
        let logs = nest_rs_testing::LogCapture::install();
        let (checked, view, _) =
            checked(&stage_of(StoreDown), &WsClient::for_test(), json!(1)).await;
        assert!(checked.is_err(), "a store that is down refuses the message");
        let Some(crate::WsReply::Error(refused)) = view.into_answer().0 else {
            panic!("a refusal answers an error frame");
        };
        assert_eq!(refused.error, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        let event = logs.expect_one("nest_rs::layers", "guard denied the message");
        assert_eq!(
            event.field("error").as_deref(),
            Some("the identity store did not answer: the identity store refused the connection"),
        );
    }

    /// What an unavailable dependency's refusal said of it stays on the server.
    #[test]
    fn an_unavailable_denial_tells_the_client_its_condition_and_its_wait_only() {
        let frame = denial_to_ws_error(Denial::unavailable(Some(7), "store at 10.0.0.1 down"));
        let json = serde_json::to_value(&frame).expect("serializes");
        assert!(!json.to_string().contains("10.0.0.1"), "{json}");
        assert_eq!(json["error"], nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(json["errors"]["reason"], "unavailable");
        assert_eq!(json["errors"]["code"], "UNAVAILABLE");
        assert_eq!(json["errors"]["retryAfterSeconds"], 7);
    }
}
