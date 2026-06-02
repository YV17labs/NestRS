//! Wire → model → mask → retain wire keys, driven through a real `#[routes]`
//! handler and the `Authorize` response shaper — no live database.

use std::sync::Arc;

use nestrs_authz::http::Authorize;
use nestrs_authz::{AbilityBuilder, Action, Read};
use nestrs_core::module;
use nestrs_http::poem::web::Json;
use nestrs_http::{async_trait, controller, routes, Guard, HttpTransport};
use nestrs_resource::WireModelDefaults;
use nestrs_testing::TestApp;
use poem::{Request, Response};
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
}

#[derive(Serialize)]
struct WidgetDto {
    id: i32,
    name: String,
}

struct AbilityInjector;

#[async_trait]
impl Guard for AbilityInjector {
    async fn check(&self, req: &mut Request) -> Result<(), Response> {
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

#[controller(path = "/widgets")]
struct WidgetController;

#[routes]
impl WidgetController {
    #[get("/:id")]
    async fn one(
        &self,
        _authz: Authorize<Read, widget::Entity>,
    ) -> Json<WidgetDto> {
        Json(WidgetDto {
            id: 1,
            name: "ada".into(),
        })
    }

    #[get("/")]
    async fn list(
        &self,
        _authz: Authorize<Read, widget::Entity>,
    ) -> Json<Vec<WidgetDto>> {
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
}

#[module(providers = [WidgetController])]
struct ShapeModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<ShapeModule>()
        .http(HttpTransport::new().guard(AbilityInjector))
        .build()
        .await
        .expect("shape harness boots")
}

#[tokio::test]
async fn a_restricted_grant_masks_to_permitted_fields() {
    let app = boot().await;
    let resp = app.http().get("/widgets/1").header("x-role", "user").send().await;
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
    let resp = app.http().get("/widgets/1").header("x-role", "admin").send().await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("secret"),
        "retain_wire_keys must drop secret even when every field is permitted: {body}",
    );
}

#[tokio::test]
async fn a_list_masks_each_row_and_retains_wire_keys() {
    struct ListAbilityInjector;

    #[async_trait]
    impl Guard for ListAbilityInjector {
        async fn check(&self, req: &mut Request) -> Result<(), Response> {
            let mut b = AbilityBuilder::new();
            b.can(Action::Read, widget::Entity);
            req.extensions_mut().insert(Arc::new(b.build()));
            Ok(())
        }
    }

    let app = TestApp::builder()
        .module::<ShapeModule>()
        .http(HttpTransport::new().guard(ListAbilityInjector))
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

    #[module(providers = [PlainController])]
    struct PlainModule;

    let app = TestApp::builder()
        .module::<PlainModule>()
        .http(HttpTransport::new().guard(AbilityInjector))
        .build()
        .await
        .expect("plain harness boots");

    let resp = app.http().get("/plain").header("x-role", "admin").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("hello").await;
}
