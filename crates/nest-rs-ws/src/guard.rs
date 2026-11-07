//! A gateway's guards: the upgrade's, run on the HTTP `GET` that opens a
//! socket, and each message's guard stage — the app-wide pool and the
//! message's own `#[use_guards]`, composed through the guards crate's
//! site-chain composition, then the posture's class gate — run by the
//! pipeline as its site's guard stage.

use std::any::TypeId;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use nest_rs_core::__private::{
    ClassGate, GuardStage, Pending, ResolvedLayer, new_unit_context, report_unresolved, shorten,
    take_pending,
};
use nest_rs_core::{Container, Failure, Handler, LayerSite, Peer, Posture, UnitContext};
use nest_rs_guards::__private::{
    GlobalBucket, GuardSpec, NeededPrincipal, Refused, SiteChainSources, carried_stage_idle,
    check_guards, client_message, compose_site_chain, denial_code, denial_of, denial_reason,
    host_chain, identifying_guard, validate_carried_chain, validate_gate_reached,
    validate_guard_chain,
};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::__private::OriginRefused;

use crate::envelope::{WsError, WsReply};
use crate::error::GatewayPostureError;
use crate::message::{DeclaredMessage, message_site};
use crate::server::WsClient;
use crate::view::WsView;

/// What a gateway's upgrade chain is named in boot logs and refusals.
fn upgrade_site(path: &str) -> String {
    format!("the {path} gateway upgrade")
}

/// What a message's guard stage composes, as `#[messages]` collects it.
pub struct MessageGuards {
    /// `#[use_guards]` on the gateway, which ran at the upgrade.
    pub upgrade: Vec<GuardSpec>,
    /// `#[use_guards]` beside the message, `#[force_guards]` included.
    pub method: Vec<GuardSpec>,
    /// `#[force_guards]`: the guards the message re-runs.
    pub force: Vec<TypeId>,
    /// The class gate the message's posture declares.
    pub gate: Option<ClassGate>,
    /// The posture withholds the ability from the message's reads: no mask
    /// shapes its reply.
    pub withheld: bool,
    /// What the message's `Ambient<T>` parameters read, by type.
    pub ambient: Vec<TypeId>,
}

/// The guard stage of the message `handler` declares: the app-wide pool and
/// the message's own guards, each once unless `#[force_guards]` replays one,
/// then its class gate — `None` when it runs none, needs no principal and has
/// nothing to withhold. The gateway's own guards ran at the upgrade, so a
/// principal a guard here requires, or a `#[public]` message reads as an
/// `Ambient<P>`, may be theirs.
pub fn message_guard_stage(
    container: &Container,
    handler: &Handler,
    declared: MessageGuards,
) -> nest_rs_core::anyhow::Result<Option<Arc<dyn GuardStage>>> {
    let site = message_site(handler);
    let sources = SiteChainSources {
        provider: Vec::new(),
        method: declared.method,
        force: declared.force,
    };
    let guards = compose_site_chain(container, &site, sources, GlobalBucket::Fold)
        .inspect_err(report_unresolved)?;
    let upgrade_chain = || host_chain(container, &declared.upgrade, &site);
    validate_carried_chain(&site, &guards, upgrade_chain)
        .map_err(nest_rs_core::anyhow::Error::msg)?;
    let needed = if handler.posture() == Posture::Public && !declared.ambient.is_empty() {
        let upgrade = upgrade_chain().map_err(nest_rs_core::anyhow::Error::msg)?;
        NeededPrincipal::of(&declared.ambient, guards.iter().chain(&upgrade))
    } else {
        None
    };
    if declared.gate.is_some() {
        validate_gate_reached(&site, &guards, upgrade_chain)
            .map_err(nest_rs_core::anyhow::Error::msg)?;
    } else if needed.is_none()
        && carried_stage_idle(&guards, declared.withheld, upgrade_chain)
            .map_err(nest_rs_core::anyhow::Error::msg)?
    {
        return Ok(None);
    }
    Ok(Some(Arc::new(MessageGuardStage {
        guards,
        needed,
        gate: declared.gate,
        withheld: declared.withheld,
    })))
}

/// A message's guards, then its class gate, as the pipeline runs a site's
/// guard stage: a refusal is answered into the message's view, so no
/// exception filter maps it.
struct MessageGuardStage {
    guards: Vec<ResolvedLayer<dyn Guard>>,
    needed: Option<NeededPrincipal>,
    gate: Option<ClassGate>,
    withheld: bool,
}

impl GuardStage for MessageGuardStage {
    fn check<'a>(&'a self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            check_guards(
                &self.guards,
                self.needed.as_ref(),
                self.gate,
                self.withheld,
                cx,
            )
            .await
            .map_err(|refused| refuse(cx, refused))
        })
    }

    fn covers(&self) -> bool {
        identifying_guard(&self.guards).is_some()
    }
}

/// The refused message's answer: a guard's denial filed, rendered into the
/// view, the unit failed with it, answered.
fn refuse(cx: &mut UnitContext<'_>, refused: Refused) -> Failure {
    let denial = refused.into_denial(DENIED, cx);
    let frame = denial_to_ws_error(denial.clone());
    match WsView::of(cx) {
        Ok(view) => Failure::from(denial).answered(view.respond(WsReply::Error(frame))),
        Err(viewless) => viewless,
    }
}

/// The line a guard's denial of a message is filed as.
const DENIED: &str = "guard denied the message";

/// Render a guard's [`Denial`] as the error frame one message answers with.
///
/// No status line: the message, its `code` and `reason`, and the
/// `requiredScopes` or the `retryAfterSeconds` it names, each a member of the
/// frame's `data`. An internal denial answers the opaque frame, the `INTERNAL`
/// code its only member, and an unavailable dependency answers its condition,
/// never what the guard said of it.
pub fn denial_to_ws_error(denial: Denial) -> WsError {
    if denial.http_status() == 500 {
        return WsError::internal();
    }
    WsError {
        required_scopes: denial.required_scopes().into(),
        retry_after_secs: denial.retry_after_secs(),
        server_failed: denial.http_status() >= 500,
        ..WsError::classed(client_message(&denial), denial_code(&denial))
            .with_reason(denial_reason(&denial))
    }
}

/// The posture's class `gate` the message `handler` declares, run alone on
/// `data`, arrived on `client` — no guard runs — as `Gateway::dispatch` runs
/// it: `data` back and what the gate attached for the handler, or the frame its
/// refusal answers, the gate filing its own.
pub(crate) fn gate_alone(
    client: &WsClient,
    handler: &'static Handler,
    gate: ClassGate,
    data: serde_json::Value,
) -> Result<(serde_json::Value, Pending), WsError> {
    let mut view = WsView::new(client.clone(), handler.address(), data);
    let peer = Peer::new(None, Some(client.id()));
    let mut cx = new_unit_context(handler, &mut view, Some(peer), None);
    let gated = gate(&mut cx);
    let attached = take_pending(&mut cx);
    match gated {
        Ok(()) => Ok((view.take_data(), attached)),
        Err(refused) => Err(denial_to_ws_error(denial_of(&refused))),
    }
}

/// The upgrade's guards of the gateway at `path`, phase-validated (an HTTP
/// `GET`, the entry the check reads) and resolvable on `container`, the pool's
/// first.
pub(crate) fn validate_upgrade(
    container: &Container,
    path: &str,
    upgrade: &[GuardSpec],
) -> nest_rs_core::anyhow::Result<Vec<ResolvedLayer<dyn Guard>>> {
    let site = upgrade_site(path);
    let chain = host_chain(container, upgrade, &site).map_err(nest_rs_core::anyhow::Error::msg)?;
    validate_guard_chain(&site, &chain).map_err(nest_rs_core::anyhow::Error::msg)?;
    Ok(chain)
}

/// HTTP's posture grammar on the gateway at `path`, whose `upgrade` chain runs
/// before any of its `messages`: one no guard identifying a caller covers says
/// `#[public]`, and one saying it is reachable with no credential. A connection
/// is a unit no message guards, so an upgrade no such guard runs at needs a
/// `#[public]` message to say so.
pub(crate) fn check_posture(
    container: &Container,
    path: &str,
    upgrade: &[ResolvedLayer<dyn Guard>],
    messages: &[DeclaredMessage],
) -> nest_rs_core::anyhow::Result<()> {
    let path = path.to_owned();
    let Some(entry) = identifying_guard(upgrade) else {
        let mut uncovered = Vec::new();
        for message in messages {
            if !is_public(message) && !message.site.chains(container)?.covers() {
                uncovered.push(message.site.handler().address());
            }
        }
        if !uncovered.is_empty() {
            let messages = uncovered.join(", ");
            return Err(GatewayPostureError::Uncovered { path, messages }.into());
        }
        if !messages.iter().any(is_public) {
            return Err(GatewayPostureError::Unopened { path }.into());
        }
        return Ok(());
    };
    let Some(messages) = events(messages, is_public) else {
        return Ok(());
    };
    let guard = shorten(entry.name);
    Err(if matches!(entry.source, LayerSite::Global) {
        GatewayPostureError::PublicBehindPool {
            path,
            messages,
            guard,
        }
    } else {
        GatewayPostureError::PublicBehindGuard {
            path,
            messages,
            guard,
        }
    }
    .into())
}

fn is_public(message: &DeclaredMessage) -> bool {
    message.site.handler().posture() == Posture::Public
}

/// The events of the `messages` that `of` picks, as a refusal lists them; `None`
/// when it picks none.
fn events(messages: &[DeclaredMessage], of: impl Fn(&DeclaredMessage) -> bool) -> Option<String> {
    let picked: Vec<&str> = messages
        .iter()
        .filter(|message| of(message))
        .map(|message| message.site.handler().address())
        .collect();
    (!picked.is_empty()).then(|| picked.join(", "))
}

/// A gateway's upgrade from a browser origin the transport's policy refuses,
/// filed on this edge's target.
pub fn refused_origin(refused: &OriginRefused) {
    nest_rs_http::__origin_refused!(target: crate::TARGET, refused);
}

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler};
    use nest_rs_core::async_trait;
    use nest_rs_core::{AmbientValue, Layer};
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
            needed: None,
            gate: None,
            withheld: false,
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

    /// With no guard at the upgrade or beside it, nothing can have attached
    /// what a bare `#[public]` withholds, so the message keeps no stage.
    #[test]
    fn a_message_with_nothing_to_withhold_pays_for_no_stage() {
        let stage = message_guard_stage(
            &Container::default(),
            &SEND,
            MessageGuards {
                upgrade: Vec::new(),
                method: Vec::new(),
                force: Vec::new(),
                gate: None,
                withheld: true,
                ambient: Vec::new(),
            },
        )
        .expect("no guard to resolve");
        assert!(stage.is_none());
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
        assert_eq!(
            serde_json::to_value(&refused).expect("a frame"),
            json!({ "error": "a number, please", "code": "FORBIDDEN", "reason": "forbidden" }),
        );
        assert!(line.is_none(), "the stage filed the denial itself");
        let event = logs.expect_one("nest_rs::layers", "guard denied the message");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("event").as_deref(), Some("send"));
        assert_eq!(event.field("unit").as_deref(), Some("ws.message"));
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
            Err(
                Denial::unavailable(None, "the identity store did not answer")
                    .with_source(ConnectionRefused),
            )
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
        assert_eq!(json["reason"], "unavailable");
        assert_eq!(json["code"], "UNAVAILABLE");
        assert_eq!(json["retryAfterSeconds"], 7);
    }

    /// An internal denial is the opaque frame every failure the client is not
    /// told of answers, its class named; every denial names the code its class
    /// is known by on every edge.
    #[test]
    fn every_denial_names_its_class_and_an_internal_one_answers_opaquely() {
        use nest_rs_guards::__private::denial_code;

        crate::testing::assert_opaque(&denial_to_ws_error(Denial::internal(
            "redis at 10.0.3.7 refused",
        )));
        for denial in [
            Denial::unauthorized("missing bearer token"),
            Denial::invalid_credential("the token expired", "invalid_token"),
            Denial::forbidden("not yours"),
            Denial::insufficient_scope(["posts:write"], "narrow"),
            Denial::rate_limited(30, "slow down"),
            Denial::unavailable(Some(7), "store at 10.0.0.1 down"),
            Denial::internal("redis at 10.0.3.7 refused"),
        ] {
            let frame = serde_json::to_value(denial_to_ws_error(denial.clone())).expect("a frame");
            assert_eq!(
                frame["code"],
                json!(denial_code(&denial).as_str()),
                "{denial:?}"
            );
            assert!(frame.get("errors").is_none(), "no field map: {frame}");
        }
        assert_eq!(
            serde_json::to_value(denial_to_ws_error(Denial::insufficient_scope(
                ["posts:write"],
                "narrow",
            )))
            .expect("a frame"),
            json!({
                "error": "narrow",
                "code": "INSUFFICIENT_SCOPE",
                "reason": "insufficient_scope",
                "requiredScopes": ["posts:write"],
            }),
        );
        assert_eq!(
            serde_json::to_value(denial_to_ws_error(Denial::rate_limited(30, "slow down")))
                .expect("a frame"),
            json!({
                "error": "slow down",
                "code": "RATE_LIMITED",
                "reason": "rate_limited",
                "retryAfterSeconds": 30,
            }),
        );
    }
}
