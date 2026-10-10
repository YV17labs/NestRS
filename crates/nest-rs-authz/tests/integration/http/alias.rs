//! A **renamed** `Authorize` alias (`use Authorize as Az`) arms the response
//! shaper exactly like the canonical spelling.
//!
//! `#[routes]` hands each parameter type to `nest_rs_http::__private::ShaperProbe`, so a
//! rename changes the spelling, not the type.

use std::sync::Arc;

use nest_rs_authz::http::Authorize as Az;
use nest_rs_authz::{AbilityBuilder, Action, Read, WireModelDefaults, current_ability};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::poem::web::Json;
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_testing::TestApp;
use poem::Request;

mod gadget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(
        Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize, schemars::JsonSchema,
    )]
    #[sea_orm(table_name = "gadgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

impl WireModelDefaults for gadget::Entity {
    fn fill_wire_defaults(map: &mut serde_json::Map<String, serde_json::Value>) {
        map.entry(String::from("secret"))
            .or_insert_with(|| serde_json::Value::String(String::new()));
    }

    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "name"])
    }
}

/// Grants `Read` on the entity when `x-grant: yes`, denies otherwise — the
/// class gate's input either way.
#[injectable]
#[derive(Default)]
struct GrantInjector;

impl Layer for GrantInjector {}

#[async_trait]
impl Guard for GrantInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let granted = req
            .headers()
            .get("x-grant")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == "yes")
            .unwrap_or(false);
        let mut b = AbilityBuilder::new();
        if granted {
            b.can(Action::Read, gadget::Entity);
        }
        req.extensions_mut()
            .insert(Arc::new(b.build().expect("valid test ability")));
        Ok(())
    }
}

impl HttpGuard for GrantInjector {}

#[controller(path = "/gadgets")]
struct GadgetController;

/// The observation rides in the entity's exposed `name`: the shaper masks
/// against the subject's wire model, so a body of any other shape would fail
/// closed — correct, but not what these tests measure.
fn probe_model(id: i32) -> gadget::Model {
    gadget::Model {
        id,
        name: format!("ambient:{}", current_ability().is_some()),
        secret: "s1".into(),
    }
}

#[routes]
impl GadgetController {
    // Aliased parameter.
    #[get("/aliased/probe")]
    async fn aliased_probe(&self, _authz: Az<Read, gadget::Entity>) -> Json<gadget::Model> {
        Json(probe_model(1))
    }

    // Literal-name control: identical posture with the canonical path.
    #[get("/literal/probe")]
    async fn literal_probe(
        &self,
        _authz: nest_rs_authz::http::Authorize<Read, gadget::Entity>,
    ) -> Json<gadget::Model> {
        Json(probe_model(2))
    }

    // Raw-`Model` body under the alias: the shaper is what strips the
    // unexposed `secret`.
    #[get("/aliased/raw")]
    async fn aliased_raw(&self, _authz: Az<Read, gadget::Entity>) -> Json<gadget::Model> {
        Json(gadget::Model {
            id: 1,
            name: "ada".into(),
            secret: "s1".into(),
        })
    }
}

#[module(providers = [GrantInjector, GadgetController])]
struct AliasModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AliasModule>()
        .use_guards_global([guard::<GrantInjector>()])
        .build()
        .await
        .expect("alias harness boots")
}

#[tokio::test]
async fn an_aliased_authorize_still_gates_at_class_level() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = boot().await;
    let denied = app.http().get("/gadgets/aliased/probe").send().await;
    assert_eq!(denied.0.status(), poem::http::StatusCode::FORBIDDEN);

    // The operator's half: the denial event an incident queries.
    let event = logs
        .find("nest_rs::authz", "authorization denied")
        .into_iter()
        .next()
        .expect("a class-level refusal reports itself on nest_rs::authz");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("transport").as_deref(), Some("http"));
    assert!(
        event.field("subject").is_some_and(|s| s.contains("gadget")),
        "the denial names the subject it refused: {event:?}",
    );
}

#[tokio::test]
async fn an_aliased_authorize_installs_the_ambient_ability() {
    let app = boot().await;

    for path in ["/gadgets/aliased/probe", "/gadgets/literal/probe"] {
        let resp = app.http().get(path).header("x-grant", "yes").send().await;
        resp.assert_status_is_ok();
        let body = resp.0.into_body().into_string().await.expect("body");
        assert!(
            body.contains("ambient:true"),
            "{path} installs the ambient ability: {body}",
        );
        assert!(
            !body.contains("secret"),
            "{path} masks the unexposed column: {body}",
        );
    }
}

#[tokio::test]
async fn an_aliased_authorize_masks_a_raw_model_body() {
    let app = boot().await;
    let resp = app
        .http()
        .get("/gadgets/aliased/raw")
        .header("x-grant", "yes")
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(body.contains("ada"), "the exposed columns ship: {body}");
    assert!(
        !body.contains("secret") && !body.contains("s1"),
        "the unexposed column is stripped: {body}",
    );
}
