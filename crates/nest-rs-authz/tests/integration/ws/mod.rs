//! Mirror tests for `src/ws/` — only compiled when the `ws` feature is on.
//!
//! A gateway is `Gated`, so these suites install the ambient ability with
//! [`install_ability`] and drive `Gateway::dispatch` directly — the class gate,
//! the handler's own call and its mask; on a socket the gate runs in a
//! message's guard stage, so the gate's suites also send their messages over a
//! real upgrade whose guard attached the ability ([`RoleUpgrade`]).

use std::sync::Arc;

use nest_rs_authz::__private::install_ability;
use nest_rs_authz::{Ability, AbilityBuilder, Action};
use nest_rs_core::{Layer, UnitContext, async_trait, injectable};
use nest_rs_guards::{Denial, Guard, GuardFor, Scope};
use nest_rs_http::HttpView;
use nest_rs_ws::{Gateway, WsClient, WsReply};

use crate::widget;

/// The header naming the role whose ability [`RoleUpgrade`] attaches.
pub(crate) const ROLE: &str = "x-role";

/// Attaches, at the upgrade, the ability of the role the request names, as an
/// app's ability guard attaches the caller's; attaches nothing without one.
#[injectable]
#[derive(Default, Layer)]
pub(crate) struct RoleUpgrade;

#[async_trait]
impl Guard for RoleUpgrade {
    fn __attaches_ability(&self) -> bool {
        true
    }

    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let role = HttpView::of(cx)?
            .request()
            .headers()
            .get(ROLE)
            .and_then(|role| role.to_str().ok())
            .map(str::to_owned);
        if let Some(role) = role {
            cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability_of(
                &role,
            )));
        }
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for RoleUpgrade {}

/// Send `event` on a socket `path` opens, upgraded as `role` — or as nobody —
/// and read its frame back.
pub(crate) async fn message_as(
    app: &nest_rs_testing::ws::WsApp,
    path: &str,
    role: Option<&str>,
    event: &str,
) -> serde_json::Value {
    let builder = app.socket(path);
    let builder = match role {
        Some(role) => builder.header(ROLE, role),
        None => builder,
    };
    let mut socket = builder.connect().await;
    socket.send(event, serde_json::Value::Null).await;
    socket.next_envelope().await
}

/// Dispatch `event` on `gateway` with `ability` installed, as the connection
/// carries it into a real message.
pub(crate) async fn dispatch_with<G: Gateway>(
    gateway: &G,
    ability: Arc<Ability>,
    event: &str,
) -> WsReply {
    install_ability(
        ability,
        gateway.dispatch(&WsClient::for_test(), event, serde_json::Value::Null),
    )
    .await
}

/// The reply's JSON, or the error frame's message.
pub(crate) fn body(reply: WsReply) -> String {
    match reply {
        WsReply::Reply(value) => value.to_string(),
        WsReply::Error(error) => format!("error: {error}"),
        WsReply::None => "none".to_owned(),
        other => format!("{other:?}"),
    }
}

/// The grants every suite here needs: unrestricted, `id`-only, `id`-only on widget
/// 1, and a story's key and its renamed column.
pub(crate) fn ability_for(role: &str) -> Arc<Ability> {
    Arc::new(ability_of(role))
}

/// [`ability_for`]'s grants, plus `scoped`: a read the credential's scopes
/// withhold.
pub(crate) fn ability_of(role: &str) -> Ability {
    if role == "scoped" {
        // `Some([])` delegated nothing; the default `None` means "not
        // scope-aware". Conflating the two is the fail-open reading.
        let mut builder = AbilityBuilder::new().with_granted_scopes(Some(Arc::from([])));
        builder
            .can(Action::Read, widget::Entity)
            .requires_scope(Scope::new("widgets:read"));
        return builder.build().expect("valid test ability");
    }
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
        "headlines" => {
            builder
                .can(Action::Read, crate::story::Entity)
                .fields([crate::story::Column::Id, crate::story::Column::Headline]);
        }
        // `nobody`: an ability that grants nothing, so the class gate refuses.
        _ => {}
    }
    builder.build().expect("valid test ability")
}
