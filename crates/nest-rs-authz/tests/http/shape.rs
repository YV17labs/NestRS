//! Wire → model → mask → retain wire keys, driven through a real `#[routes]`
//! handler and the `Authorize` response shaper — no live database.

use std::sync::Arc;

use nest_rs_authz::http::Authorize;
use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::poem::web::Json;
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_resource::WireModelDefaults;
use nest_rs_testing::TestApp;
use poem::Request;
use serde::Serialize;

mod widget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize)]
    #[sea_orm(table_name = "widgets")]
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

impl WireModelDefaults for widget::Entity {
    fn fill_wire_defaults(map: &mut serde_json::Map<String, serde_json::Value>) {
        map.entry(String::from("secret"))
            .or_insert_with(|| serde_json::Value::String(String::new()));
    }

    // `secret` is unexposed — mirrors what `#[expose]` emits for a real entity,
    // so the masker strains against the static set rather than the body keys.
    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "name"])
    }
}

#[derive(Serialize)]
struct WidgetDto {
    id: i32,
    name: String,
}

#[injectable]
#[derive(Default)]
struct AbilityInjector;

impl Layer for AbilityInjector {}

#[async_trait]
impl Guard for AbilityInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let admin = req
            .headers()
            .get("x-role")
            .and_then(|v| v.to_str().ok())
            .map(|role| role == "admin")
            .unwrap_or(false);
        let mut b = AbilityBuilder::new();
        if admin {
            b.can(Action::Read, widget::Entity)
                .when(|p| p.eq(widget::Column::Id, 1));
        } else {
            b.can(Action::Read, widget::Entity)
                .when(|p| p.eq(widget::Column::Id, 1))
                .fields([widget::Column::Name]);
        }
        req.extensions_mut().insert(Arc::new(b.build()));
        Ok(())
    }
}

#[injectable]
#[derive(Default)]
struct ListAbilityInjector;

impl Layer for ListAbilityInjector {}

#[async_trait]
impl Guard for ListAbilityInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        req.extensions_mut().insert(Arc::new(b.build()));
        Ok(())
    }
}

#[controller(path = "/widgets")]
struct WidgetController;

#[routes]
impl WidgetController {
    #[get("/:id")]
    async fn one(&self, _authz: Authorize<Read, widget::Entity>) -> Json<WidgetDto> {
        Json(WidgetDto {
            id: 1,
            name: "ada".into(),
        })
    }

    #[get("/")]
    async fn list(&self, _authz: Authorize<Read, widget::Entity>) -> Json<Vec<WidgetDto>> {
        Json(vec![
            WidgetDto {
                id: 1,
                name: "ada".into(),
            },
            WidgetDto {
                id: 2,
                name: "bob".into(),
            },
        ])
    }

    // A handler that (incorrectly) returns a raw `Model` carrying the unexposed
    // `secret`. The shaper must still strip it: masking keys on the entity's
    // static wire-key set, not on whatever the body shipped.
    #[get("/raw/one")]
    async fn raw_one(&self, _authz: Authorize<Read, widget::Entity>) -> Json<widget::Model> {
        Json(widget::Model {
            id: 1,
            name: "ada".into(),
            secret: "s1".into(),
        })
    }

    // A raw-`Model` list where the ability drops row id=2 (so `mask_many`
    // returns fewer rows than the body) under an unrestricted grant on id=1.
    // The dropped-row branch must still strip `secret` from the survivor.
    #[get("/raw/list")]
    async fn raw_list(&self, _authz: Authorize<Read, widget::Entity>) -> Json<Vec<widget::Model>> {
        Json(vec![
            widget::Model {
                id: 1,
                name: "ada".into(),
                secret: "s1".into(),
            },
            widget::Model {
                id: 2,
                name: "bob".into(),
                secret: "s2".into(),
            },
        ])
    }
}

#[module(providers = [AbilityInjector, ListAbilityInjector, WidgetController])]
struct ShapeModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<ShapeModule>()
        .use_guards_global([guard::<AbilityInjector>()])
        .build()
        .await
        .expect("shape harness boots")
}

#[tokio::test]
async fn a_restricted_grant_masks_to_permitted_fields() {
    let app = boot().await;
    let resp = app
        .http()
        .get("/widgets/1")
        .header("x-role", "user")
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body)
            .expect("json")
            .get("name")
            .and_then(|v| v.as_str()),
        Some("ada"),
    );
    assert!(
        !body.contains("secret"),
        "secret must be stripped from the wire body: {body}",
    );
}

#[tokio::test]
async fn an_unrestricted_grant_cannot_leak_skipped_columns() {
    let app = boot().await;
    let resp = app
        .http()
        .get("/widgets/1")
        .header("x-role", "admin")
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("secret"),
        "masking must drop secret even when every field is permitted: {body}",
    );
}

#[tokio::test]
async fn a_raw_model_handler_cannot_leak_unexposed_columns() {
    // Regression: a handler returning `Json(Model)` instead of the wire DTO must
    // not leak the unexposed `secret`, even under an unrestricted (admin) grant.
    let app = boot().await;
    let resp = app
        .http()
        .get("/widgets/raw/one")
        .header("x-role", "admin")
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("secret") && !body.contains("s1"),
        "a raw-Model body must be cut down to exposed columns: {body}",
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body)
            .expect("json")
            .get("name")
            .and_then(|v| v.as_str()),
        Some("ada"),
        "exposed columns survive: {body}",
    );
}

#[tokio::test]
async fn a_dropped_row_does_not_leak_unexposed_columns() {
    // Regression: when `mask_many` drops a row (id=2 denied) under an
    // unrestricted grant on id=1, the survivor must still be stripped — the
    // dropped-row branch previously skipped the wire-key strainer.
    let app = boot().await;
    let resp = app
        .http()
        .get("/widgets/raw/list")
        .header("x-role", "admin")
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("secret") && !body.contains("s1") && !body.contains("s2"),
        "dropped-row masking must still strip unexposed columns: {body}",
    );
    let rows: serde_json::Value = serde_json::from_str(&body).expect("json array");
    assert_eq!(
        rows.as_array().map(|r| r.len()),
        Some(1),
        "only the permitted row survives: {body}",
    );
}

#[tokio::test]
async fn a_list_masks_each_row_and_retains_wire_keys() {
    let app = TestApp::builder()
        .module::<ShapeModule>()
        .use_guards_global([guard::<ListAbilityInjector>()])
        .build()
        .await
        .expect("list harness boots");

    let resp = app.http().get("/widgets").send().await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("secret"),
        "secret never appears on the wire: {body}",
    );
}

#[tokio::test]
async fn a_non_json_response_passes_through() {
    #[controller(path = "/plain")]
    struct PlainController;

    #[routes]
    impl PlainController {
        #[get("/")]
        async fn plain(&self, _authz: Authorize<Read, widget::Entity>) -> String {
            "hello".into()
        }
    }

    #[module(providers = [AbilityInjector, PlainController])]
    struct PlainModule;

    let app = TestApp::builder()
        .module::<PlainModule>()
        .use_guards_global([guard::<AbilityInjector>()])
        .build()
        .await
        .expect("plain harness boots");

    let resp = app
        .http()
        .get("/plain")
        .header("x-role", "admin")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("hello").await;
}
