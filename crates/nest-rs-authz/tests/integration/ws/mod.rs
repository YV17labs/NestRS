//! Mirror tests for `src/ws/` — only compiled when the `ws` feature is on.
//!
//! A gateway is `EdgePosture::Guarded`, so there is no bridge to boot and no
//! in-band chain to re-run: the upgrade already carried the real HTTP guards, and
//! what a message needs is the ambient ability its `SocketContext` re-installs.
//! These suites therefore install it with [`with_ability`] and drive
//! `Gateway::dispatch` directly — the same seam `nest-rs-ws`'s own tests use, and
//! the one the connection loop calls.

mod authorize;
mod mask;

use std::sync::Arc;

use nest_rs_authz::{Ability, AbilityBuilder, Action, with_ability};
use nest_rs_ws::{Gateway, WsClient, WsReply};

use crate::widget;

/// Dispatch `event` on `gateway` with `ability` installed, exactly as
/// `WsDataContext` would around a real message.
pub(crate) async fn dispatch_with<G: Gateway>(
    gateway: &G,
    ability: Arc<Ability>,
    event: &str,
) -> WsReply {
    with_ability(
        ability,
        gateway.dispatch(&WsClient::for_test(), event, serde_json::Value::Null),
    )
    .await
}

/// The reply's JSON, or the error frame's message — one accessor so a suite reads
/// the outcome without re-matching `WsReply` at every assertion.
pub(crate) fn body(reply: WsReply) -> String {
    match reply {
        WsReply::Reply(value) => value.to_string(),
        WsReply::Error(error) => format!("error: {error}"),
        WsReply::None => "none".to_owned(),
    }
}

/// The three grants every suite here needs: unrestricted, `id`-only, and
/// `id`-only on widget 1. Built by name so the suites read as roles rather than
/// as builder calls.
pub(crate) fn ability_for(role: &str) -> Arc<Ability> {
    let mut builder = AbilityBuilder::new();
    match role {
        "admin" => {
            builder.can(Action::Read, widget::Entity);
        }
        "viewer" => {
            builder
                .can(Action::Read, widget::Entity)
                .fields([widget::Column::Id]);
        }
        "auditor" => {
            builder
                .can(Action::Read, widget::Entity)
                .when(|p| p.eq(widget::Column::Id, 1))
                .fields([widget::Column::Id]);
        }
        // `nobody`: an ability that grants nothing, so the class gate refuses.
        _ => {}
    }
    Arc::new(builder.build().expect("valid test ability"))
}
