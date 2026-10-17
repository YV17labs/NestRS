//! A gateway's guards: the upgrade's, run on the HTTP `GET` that opens a
//! socket, and each `#[subscribe_message]` event's per-message chain, composed
//! and deduped once at gateway mount through the `compose_chain` dedup every
//! other site uses.
//!
//! `#[messages]`' boot check refuses a declaration no imported module provides
//! before the gateway mounts; the mount itself, which has no `Result`, files
//! the same refusal and denies, so it never runs a chain without a layer.

use std::any::TypeId;
use std::collections::HashMap;

use nest_rs_core::__private::{
    ClassGate, Pending, ResolvedLayer, compose_chain, dedup_bucket, new_unit_context,
    refuse_unresolved, resolve_scoped,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, Handler, Peer};
use nest_rs_guards::__private::{
    boot_validate_guards, client_message, denial_of, failure_chain, resolve_global_guards,
    run_stage, structured_reason,
};
use nest_rs_guards::{Denial, Guard, GuardSpec};
use nest_rs_http::__private::OriginRefused;

use crate::envelope::WsError;
use crate::server::WsClient;
use crate::view::WsView;

/// What an event's chain is named in boot logs and refusals.
fn event_site(event: &str) -> String {
    format!("ws {event}")
}

/// What a gateway's upgrade chain is named in boot logs and refusals.
fn upgrade_site(path: &str) -> String {
    format!("the {path} gateway upgrade")
}

/// One event's per-message chain, as the gateway runs it.
pub enum EventChain {
    /// The app-wide pool and the event's own `#[use_guards]`, deduped, run as
    /// the unit its message's `handler` declares.
    Composed {
        /// The event's declaration.
        handler: &'static Handler,
        /// The guards, in the order they run.
        guards: Vec<ResolvedLayer<dyn Guard>>,
    },
    /// The chain did not compose: every message of the event is refused,
    /// opaquely.
    Refused,
}

/// Per-gateway event-name → guard chain, built once at mount by `#[messages]`.
#[derive(Default)]
pub struct EventLayerTable {
    by_event: HashMap<&'static str, EventChain>,
}

impl EventLayerTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the resolved guard chain for one event name.
    pub fn insert(&mut self, event: &'static str, chain: EventChain) {
        self.by_event.insert(event, chain);
    }

    /// The event a handler serves under `event`, and the unit its message is
    /// declared as — `None` for an event no handler serves, or one whose
    /// chain did not compose.
    pub fn handler(&self, event: &str) -> Option<(&'static str, &'static Handler)> {
        match self.by_event.get_key_value(event)? {
            (&event, EventChain::Composed { handler, .. }) => Some((event, *handler)),
            (_, EventChain::Refused) => None,
        }
    }

    /// Run every guard in the chain for `event` on its message, in order: what
    /// they attached and the message's `data` back when they all admit it — at
    /// once for an event with no chain — the first refusal's frame otherwise.
    pub async fn check(
        &self,
        client: &WsClient,
        event: &str,
        data: serde_json::Value,
    ) -> Result<(Pending, serde_json::Value), WsError> {
        let Some((&event, chain)) = self.by_event.get_key_value(event) else {
            return Ok((Pending::default(), data));
        };
        let (handler, guards) = match chain {
            EventChain::Composed { handler, guards } => (*handler, guards),
            EventChain::Refused => {
                return Err(denial_to_ws_error(Denial::internal(
                    "the event's guard chain did not compose",
                )));
            }
        };
        if guards.is_empty() {
            return Ok((Pending::default(), data));
        }
        let mut view = WsView::new(client.clone(), event, data);
        let peer = Peer::new(None, Some(client.id()));
        let mut cx = new_unit_context(handler, &mut view, Some(peer), None);
        let attached = run_stage(guards, None, &mut cx).await.map_err(|refused| {
            denial_to_ws_error(refused.into_denial(|guard, denial| {
                // Structural floor mirroring the HTTP edge's, filed where the
                // denial is still held; its text never reaches the line.
                tracing::warn!(
                    target: nest_rs_core::target::LAYERS,
                    guard,
                    conn_id = client.id(),
                    event,
                    status = denial.http_status(),
                    error = failure_chain(denial),
                    "guard denied the message",
                );
            }))
        })?;
        Ok((attached, view.take_data()))
    }
}

/// Run the posture's class `gate` on a message `client` sent to the event
/// `handler` declares, before its payload is read: the refusal's error frame,
/// the gate's own line filed.
pub fn class_gate(
    client: &WsClient,
    handler: &'static Handler,
    gate: ClassGate,
) -> Result<(), WsError> {
    let mut view = WsView::piped(client.clone(), handler.address());
    let peer = Peer::new(None, Some(client.id()));
    let mut cx = new_unit_context(handler, &mut view, Some(peer), None);
    gate(&mut cx).map_err(|refused| denial_to_ws_error(denial_of(&refused)))
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

/// The boot check `#[messages]` emits for a gateway: its upgrade chain is
/// phase-validated (an HTTP `GET`, the entry the check reads), and a guard no
/// imported module provides — on the upgrade or on any event — refuses, named
/// with its site. A message deadline the shutdown window cannot hold is said
/// at `warn`.
pub fn check_ws_gateway(
    container: &Container,
    path: &str,
    upgrade: &[GuardSpec],
    events: &[(&'static str, Vec<GuardSpec>)],
) -> Result<(), String> {
    boot_validate_guards(container, upgrade, &upgrade_site(path))?;
    for (event, method) in events {
        resolve_scoped(container, method, LayerSite::Method, &event_site(event))
            .map_err(|unresolved| unresolved.to_string())?;
    }
    let window = container
        .get::<nest_rs_http::HttpConfig>()
        .unwrap_or_default()
        .shutdown_timeout;
    container
        .get::<crate::WsConfig>()
        .unwrap_or_default()
        .warn_when_messages_outlast_the_window(window, path);
    Ok(())
}

/// One event's per-message chain: the app-wide pool and the event's own
/// `#[use_guards]`, deduped unless `#[force_guards]` replays one, run as the
/// unit `handler` declares.
pub fn ws_event_chain(
    container: &Container,
    method: &[GuardSpec],
    force: &[TypeId],
    event: &'static str,
    handler: &'static Handler,
) -> EventChain {
    let site = event_site(event);
    let method = match resolve_scoped(container, method, LayerSite::Method, &site) {
        Ok(method) => method,
        Err(unresolved) => {
            refuse_unresolved(container, unresolved);
            return EventChain::Refused;
        }
    };
    let global = dedup_bucket(resolve_global_guards(container));
    EventChain::Composed {
        handler,
        guards: compose_chain::<dyn Guard>(global, Vec::new(), method, force, &site),
    }
}

/// A gateway's upgrade from a browser origin the transport's policy refuses,
/// filed on this edge's target.
pub fn refused_origin(refused: &OriginRefused) {
    tracing::warn!(
        target: crate::TARGET,
        origin = refused.origin(),
        expected = refused.expected(),
        "browser origin refused",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler};
    use nest_rs_core::{AmbientValue, Layer, Posture, UnitContext};
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

    fn table() -> EventLayerTable {
        let mut table = EventLayerTable::new();
        table.insert(
            "send",
            EventChain::Composed {
                handler: &SEND,
                guards: vec![ResolvedLayer {
                    type_id: TypeId::of::<Numbers>(),
                    name: "Numbers",
                    source: LayerSite::Method,
                    layer: Arc::new(Numbers),
                }],
            },
        );
        table.insert("broken", EventChain::Refused);
        table
    }

    #[tokio::test]
    async fn an_unguarded_event_passes_its_data_through() {
        let (attached, data) = table()
            .check(&WsClient::for_test(), "anything", json!("text"))
            .await
            .expect("no chain, no refusal");
        assert!(attached.is_empty());
        assert_eq!(data, json!("text"));
    }

    #[tokio::test]
    async fn a_guard_reads_the_message_and_its_connection_and_attaches_for_the_handler() {
        let client = WsClient::for_test();
        let (attached, data) = table()
            .check(&client, "send", json!(7))
            .await
            .expect("a number is admitted");
        assert_eq!(data, json!(7), "the handler gets the data back");
        assert_eq!(
            attached.get::<Seen>().as_deref(),
            Some(&Seen("send", client.id())),
        );
    }

    #[tokio::test]
    async fn a_denial_answers_the_frame_and_is_filed() {
        let logs = nest_rs_testing::LogCapture::install();
        let Err(refused) = table()
            .check(&WsClient::for_test(), "send", json!("text"))
            .await
        else {
            panic!("text is refused");
        };
        assert_eq!(refused.error, "a number, please");
        assert_eq!(
            refused.errors.as_ref().map(|e| &e["code"]),
            Some(&json!("FORBIDDEN"))
        );
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
        let mut table = EventLayerTable::new();
        table.insert(
            "send",
            EventChain::Composed {
                handler: &SEND,
                guards: vec![ResolvedLayer {
                    type_id: TypeId::of::<StoreDown>(),
                    name: "StoreDown",
                    source: LayerSite::Method,
                    layer: Arc::new(StoreDown),
                }],
            },
        );
        let Err(refused) = table.check(&WsClient::for_test(), "send", json!(1)).await else {
            panic!("a store that is down refuses the message");
        };
        assert_eq!(refused.error, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        let event = logs.expect_one("nest_rs::layers", "guard denied the message");
        assert_eq!(
            event.field("error").as_deref(),
            Some("the identity store did not answer: the identity store refused the connection"),
        );
    }

    #[tokio::test]
    async fn an_event_whose_chain_did_not_compose_refuses_every_message_opaquely() {
        let Err(refused) = table()
            .check(&WsClient::for_test(), "broken", json!(1))
            .await
        else {
            panic!("an event whose chain did not compose is refused");
        };
        assert_eq!(refused.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE);
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
