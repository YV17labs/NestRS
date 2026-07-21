//! Pins the current behavior of a **renamed** `Authorize` alias
//! (`use Authorize as Az`) — the divergence `extractor.rs` documents: the
//! class-level gate still runs (extraction is type-based), and `#[routes]` arms
//! the response shaper by *textual* path-segment match, so the shaper is NOT
//! armed under a rename.
//!
//! Crucially, that is no longer a silent leak: an unarmed route carries a
//! `MaskProbe`, so a masking extractor (`Az`) running without an armed shaper
//! **fails closed** — the response becomes a logged `500` instead of an
//! unmasked body. These tests pin that fail-closed runtime net. When the
//! ambient-context seam rework makes arming alias-proof, the aliased routes
//! will succeed *and* mask, and these assertions flip again.

use std::sync::Arc;

use nest_rs_authz::http::Authorize as Az;
use nest_rs_authz::{AbilityBuilder, Action, Read, current_ability};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::poem::web::Json;
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_resource::WireModelDefaults;
use nest_rs_testing::TestApp;
use poem::Request;
use serde::Serialize;

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

#[derive(Serialize, schemars::JsonSchema)]
struct Probe {
    ambient_ability: bool,
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

#[controller(path = "/gadgets")]
struct GadgetController;

#[routes]
impl GadgetController {
    // Aliased parameter: the gate runs, the shaper does not.
    #[get("/aliased/probe")]
    async fn aliased_probe(&self, _authz: Az<Read, gadget::Entity>) -> Json<Probe> {
        Json(Probe {
            ambient_ability: current_ability().is_some(),
        })
    }

    // Literal-name control: identical posture with the canonical path — the
    // shaper installs, so the handler observes the ambient ability. The
    // observation rides in the entity's exposed `name` (a `Probe` body would be
    // irreconcilable with the subject's wire model and fail closed — correct,
    // but not what this test measures).
    #[get("/literal/probe")]
    async fn literal_probe(
        &self,
        _authz: nest_rs_authz::http::Authorize<Read, gadget::Entity>,
    ) -> Json<gadget::Model> {
        Json(gadget::Model {
            id: 1,
            name: format!("ambient:{}", current_ability().is_some()),
            secret: "s1".into(),
        })
    }

    // Raw-Model body under the alias — with the shaper skipped, nothing strips
    // the unexposed `secret`.
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
async fn an_aliased_authorize_fails_closed_instead_of_running_unshaped() {
    // Under a rename the shaper is not armed, so the ambient ability is not
    // installed. Rather than run the masking extractor unshaped (which would
    // once have shipped an unmasked body), the `MaskProbe` fails the response
    // closed with a `500`. The literal-name control still succeeds and masks.
    let app = boot().await;

    let aliased = app
        .http()
        .get("/gadgets/aliased/probe")
        .header("x-grant", "yes")
        .send()
        .await;
    assert_eq!(
        aliased.0.status(),
        poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        "an unarmed masking extractor (aliased Authorize) must fail closed, not run unshaped",
    );

    let literal = app
        .http()
        .get("/gadgets/literal/probe")
        .header("x-grant", "yes")
        .send()
        .await;
    literal.assert_status_is_ok();
    let body = literal.0.into_body().into_string().await.expect("body");
    assert!(
        body.contains("ambient:true"),
        "the canonical path installs the ambient ability: {body}",
    );
    assert!(
        !body.contains("secret"),
        "and the shaper masks the unexposed column: {body}",
    );
}

#[tokio::test]
async fn an_aliased_authorize_fails_closed_rather_than_shipping_an_unmasked_body() {
    // KNOWN GAP with a fail-closed net: the shaper is skipped under a rename, so
    // field-masking cannot run — but rather than ship the unexposed `secret`,
    // the `MaskProbe` turns the response into a `500`. Tracked for the
    // ambient-context seam rework, which will make the aliased route succeed AND
    // mask (flip this to a masked `200` then).
    let app = boot().await;
    let resp = app
        .http()
        .get("/gadgets/aliased/raw")
        .header("x-grant", "yes")
        .send()
        .await;
    assert_eq!(
        resp.0.status(),
        poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        "an unarmed masking extractor must fail closed, never ship the raw unmasked body",
    );
}
