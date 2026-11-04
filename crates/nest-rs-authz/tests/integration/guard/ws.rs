//! `AbilityGuard`'s unit entry on a WebSocket message (`src/guard.rs`), through the
//! real decorators: a `#[public]` message under an upgrade no guard runs at reads
//! the visitor's rules, and a gated one still fails closed.

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Read};
use nest_rs_core::{AmbientValue, injectable, module};
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsError, WsModule, gateway, messages};

use super::rules_read;
use crate::widget;
use crate::ws::message_as;

const PATH: &str = "/ws/guard/visitor";

#[derive(Clone)]
struct Actor;

impl AmbientValue for Actor {}

#[injectable]
#[derive(Default)]
struct Rules;

impl AbilityFactory for Rules {
    type Actor = Actor;

    fn define(&self, _actor: &Self::Actor, ability: &mut AbilityBuilder) {
        ability.can(Action::Read, widget::Entity);
    }

    fn define_visitor(&self, ability: &mut AbilityBuilder) {
        ability.can(Action::Delete, widget::Entity);
    }
}

type AuthzGuard = AbilityGuard<Rules>;

#[gateway(path = "/ws/guard/visitor")]
struct VisitorGateway;

#[messages]
impl VisitorGateway {
    #[subscribe_message("open")]
    #[public]
    #[use_guards(AuthzGuard)]
    async fn open(&self) -> String {
        rules_read()
    }

    #[subscribe_message("gated")]
    #[authorize(Read, widget::Entity, unmasked)]
    #[use_guards(AuthzGuard)]
    async fn gated(&self) -> Result<String, WsError> {
        Ok("reached".to_owned())
    }
}

#[module(imports = [WsModule], providers = [Rules, AuthzGuard, VisitorGateway])]
struct VisitorModule;

#[tokio::test]
async fn a_public_message_beside_the_ability_guard_reads_the_visitors_rules() {
    let app = TestApp::builder()
        .module::<VisitorModule>()
        .build_ws()
        .await
        .expect("a public message beside the ability guard boots");

    let open = message_as(&app, PATH, None, "open").await;
    assert_eq!(open["data"], "visitor may delete", "{open}");

    let gated = message_as(&app, PATH, None, "gated").await;
    assert_eq!(gated["data"]["code"], "UNAUTHENTICATED", "{gated}");
    assert!(!gated.to_string().contains("reached"), "{gated}");

    app.shutdown().await.expect("the transport stops cleanly");
}
