//! `AbilityGuard`'s unit entry on a WebSocket message (`src/guard.rs`), through the
//! real decorators: a `#[public]` message under an upgrade no guard runs at reads
//! the visitor's rules, and a gated one refuses the boot, posture stated or not.

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Delete, Read};
use nest_rs_core::{AmbientValue, injectable, module};
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsError, WsModule, gateway, messages};

use super::{assert_refused_at_the_request, rules_read};
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
    #[public(Delete, widget::Entity, unmasked)]
    #[use_guards(AuthzGuard)]
    async fn open(&self) -> String {
        rules_read()
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

    app.shutdown().await.expect("the transport stops cleanly");
}

#[gateway(path = "/ws/guard/gated")]
struct GatedAtTheUnitGateway;

#[messages]
impl GatedAtTheUnitGateway {
    #[subscribe_message("open")]
    #[public(Delete, widget::Entity, unmasked)]
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

#[module(imports = [WsModule], providers = [Rules, AuthzGuard, GatedAtTheUnitGateway])]
struct GatedAtTheUnitModule;

/// The guard builds an ability at a `#[public]` message alone: a gated one reads what
/// its upgrade attached, so with nothing attaching it there every call would answer
/// `401`.
#[tokio::test]
async fn a_gated_message_whose_ability_guard_sits_at_the_unit_alone_refuses_the_boot() {
    assert_refused_at_the_request(
        TestApp::builder()
            .module::<GatedAtTheUnitModule>()
            .build_ws()
            .await,
        "GatedAtTheUnitGateway::gated",
    );
}

#[gateway(path = "/ws/guard/undeclared")]
struct UndeclaredAtTheUnitGateway;

#[messages]
impl UndeclaredAtTheUnitGateway {
    #[subscribe_message("open")]
    #[public(Delete, widget::Entity, unmasked)]
    #[use_guards(AuthzGuard)]
    async fn open(&self) -> String {
        rules_read()
    }

    #[subscribe_message("listed")]
    #[use_guards(AuthzGuard)]
    async fn listed(&self) -> Result<String, WsError> {
        Ok("reached".to_owned())
    }
}

#[module(imports = [WsModule], providers = [Rules, AuthzGuard, UndeclaredAtTheUnitGateway])]
struct UndeclaredAtTheUnitModule;

/// A message stating no posture is gated by its guards, so the ability guard there reads
/// what its upgrade attached, as on an `#[authorize]` one.
#[tokio::test]
async fn a_message_stating_no_posture_whose_ability_guard_sits_at_the_unit_alone_refuses_the_boot()
{
    assert_refused_at_the_request(
        TestApp::builder()
            .module::<UndeclaredAtTheUnitModule>()
            .build_ws()
            .await,
        "UndeclaredAtTheUnitGateway::listed",
    );
}
