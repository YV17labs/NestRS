//! Pins the current behavior of a **renamed** `Authorize` alias
//! (`use Authorize as Az`) — the divergence `extractor.rs` documents: the
//! class-level gate still runs (extraction is type-based), but `#[routes]`
//! arms the response shaper by *textual* path-segment match, so the ambient
//! ability install and response masking are silently skipped under a rename.
//!
//! These tests lock that divergence in place until the ambient-context seam
//! rework (owner decision: option (b), post-launch) makes arming alias-proof —
//! at which point the "skips"/"unmasked" assertions below must flip.

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
async fn an_aliased_authorize_skips_the_ambient_ability_install() {
    // The shaper is armed by textual match, so the alias route runs without an
    // ambient ability — which is exactly why the scoped data path stays
    // fail-closed (a request-scoped executor with no ambient ability denies
    // every row; pinned in nest-rs-seaorm's
    // `request_scope_without_ability_denies_all_rows`).
    let app = boot().await;

    let aliased = app
        .http()
        .get("/gadgets/aliased/probe")
        .header("x-grant", "yes")
        .send()
        .await;
    aliased.assert_status_is_ok();
    aliased
        .assert_json(&Probe {
            ambient_ability: false,
        })
        .await;

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
async fn an_aliased_authorize_leaves_the_body_unmasked() {
    // KNOWN GAP, deliberately pinned: with the shaper skipped, field-masking
    // does not run, so a handler that builds its body outside the scoped
    // executor ships unexposed columns. Tracked for the ambient-context seam
    // rework (B4 option (b)); that fix must flip this assertion.
    let app = boot().await;
    let resp = app
        .http()
        .get("/gadgets/aliased/raw")
        .header("x-grant", "yes")
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        body.contains("secret"),
        "current behavior: the alias route ships the raw body unmasked \
         (flip this when arming becomes alias-proof): {body}",
    );
}
