//! A **renamed** `Authorize` alias (`use Authorize as Az`) arms the response
//! shaper exactly like the canonical spelling.
//!
//! `#[routes]` no longer scans parameter *names*: it hands each parameter type
//! to `nest_rs_http::ShaperProbe` and the compiler answers whether that type is
//! a `RouteResponseShaper`. A rename changes the spelling, not the type — so
//! the class gate, the ambient ability and the field mask all land under an
//! alias, and the `MaskProbe` `500` these tests used to pin is unreachable from
//! here.

use std::sync::Arc;

use nest_rs_authz::http::Authorize as Az;
use nest_rs_authz::{AbilityBuilder, Action, Read, current_ability};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::poem::web::Json;
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_resource::WireModelDefaults;
use nest_rs_testing::TestApp;
use poem::Request;

mod gadget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(
        Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize, schemars::JsonSchema,
    )]
    #[sea_orm(table_name = "gadgets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

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
    // Extraction resolves the *type*, not its written name, so the 403 gate
    // survives a rename.
    let app = boot().await;
    let denied = app.http().get("/gadgets/aliased/probe").send().await;
    assert_eq!(denied.0.status(), poem::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_aliased_authorize_installs_the_ambient_ability() {
    // Arming is type-directed, so the alias and the canonical spelling reach
    // the same shaper and both install the ambient ability the data layer
    // scopes its queries by.
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
    // The case that used to be the known gap: under a rename the shaper was not
    // armed, and the run-time probe turned the response into a `500` rather
    // than ship `secret`. It now masks and returns a `200`.
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
