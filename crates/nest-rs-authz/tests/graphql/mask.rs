//! Automatic response masking through the real macros: `#[authorize(Action,
//! Entity)]` on a `#[query]` makes `#[resolver]` emit `masked_value_for`
//! around the return value — the resolver body writes no masking call. Covers
//! the wire shapes the wrapper sees through (bare DTO, `Vec`), the fail-closed
//! path (a required field the mask strips ⇒ GraphQL error, never unmasked
//! data), and the `#[public]` opt-out.

use std::sync::Arc;

use nest_rs_authz::graphql::GraphqlAbilityBridge;
use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::{Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlModule, GraphqlOperationGuard, resolver};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::async_trait;
use nest_rs_http::poem::Request;
use nest_rs_resource::WireModelDefaults;
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};

/// A throwaway SeaORM entity with a server-only column (`secret`) the wire
/// DTOs never carry — [`WireModelDefaults`] reconstructs it for policy and the
/// exposed-key strainer drops it again.
mod widget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
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
        map.entry("secret")
            .or_insert(serde_json::Value::String(String::new()));
    }

    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "name"])
    }
}

/// The wire shape: `name` optional so a field-restricted mask yields `None`
/// rather than an irreconcilable value.
#[derive(SimpleObject, Serialize, Deserialize)]
struct WidgetDto {
    id: i32,
    name: Option<String>,
}

/// A wire shape with a **required** `name`: when the mask strips it, the
/// masked value can no longer be deserialized — the operation must fail
/// closed.
#[derive(SimpleObject, Serialize, Deserialize)]
struct StrictWidgetDto {
    id: i32,
    name: String,
}

/// No-op stand-in for the bridge's authentication slot.
#[injectable]
#[derive(Default)]
struct PassGuard;

impl Layer for PassGuard {}

#[async_trait]
impl Guard for PassGuard {}

/// Builds the caller's ability from an `x-role` header: `admin` reads widgets
/// unrestricted; `viewer` reads widgets but only the `id` field; anyone else
/// gets nothing.
#[injectable]
#[derive(Default)]
struct AbilityInjector;

impl Layer for AbilityInjector {}

#[async_trait]
impl Guard for AbilityInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let role = req
            .headers()
            .get("x-role")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let mut b = AbilityBuilder::new();
        match role.as_str() {
            "admin" => {
                b.can(Action::Read, widget::Entity);
            }
            "viewer" => {
                b.can(Action::Read, widget::Entity)
                    .fields([widget::Column::Id]);
            }
            _ => {}
        }
        req.extensions_mut().insert(Arc::new(b.build()));
        Ok(())
    }
}

#[resolver]
struct MaskResolver;

#[resolver]
impl MaskResolver {
    #[query]
    #[authorize(Read, widget::Entity)]
    async fn widget(&self) -> GqlResult<WidgetDto> {
        Ok(WidgetDto {
            id: 1,
            name: Some("ada".into()),
        })
    }

    #[query]
    #[authorize(Read, widget::Entity)]
    async fn widgets(&self) -> GqlResult<Vec<WidgetDto>> {
        Ok(vec![
            WidgetDto {
                id: 1,
                name: Some("ada".into()),
            },
            WidgetDto {
                id: 2,
                name: Some("grace".into()),
            },
        ])
    }

    #[query]
    #[authorize(Read, widget::Entity)]
    async fn strict_widget(&self) -> GqlResult<StrictWidgetDto> {
        Ok(StrictWidgetDto {
            id: 1,
            name: "ada".into(),
        })
    }

    #[query]
    #[public]
    async fn motd(&self) -> GqlResult<String> {
        Ok("hello".into())
    }
}

type TestOpGuard = GraphqlAbilityBridge<PassGuard, AbilityInjector>;

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        PassGuard,
        AbilityInjector,
        TestOpGuard as dyn GraphqlOperationGuard,
        MaskResolver,
    ],
)]
struct MaskGraphqlModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<MaskGraphqlModule>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

async fn query(app: &TestApp, role: &str, query: &str) -> serde_json::Value {
    let mut req = app.http().post("/graphql");
    if !role.is_empty() {
        req = req.header("x-role", role);
    }
    let resp = req
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    resp.assert_status_is_ok();
    serde_json::to_value(resp.json().await).expect("a GraphQL response is JSON")
}

#[tokio::test]
async fn unrestricted_caller_sees_the_field() {
    let app = boot().await;
    let json = query(&app, "admin", "{ widget { id name } }").await;
    assert_eq!(json["data"]["widget"]["id"], 1);
    assert_eq!(json["data"]["widget"]["name"], "ada");
}

#[tokio::test]
async fn restricted_field_is_masked_to_null() {
    let app = boot().await;
    let json = query(&app, "viewer", "{ widget { id name } }").await;
    assert_eq!(json["data"]["widget"]["id"], 1);
    assert_eq!(
        json["data"]["widget"]["name"],
        serde_json::Value::Null,
        "the resolver returned Some(\"ada\") — the emitted mask must strip it"
    );
}

#[tokio::test]
async fn every_row_of_a_vec_is_masked() {
    let app = boot().await;
    let json = query(&app, "viewer", "{ widgets { id name } }").await;
    let rows = json["data"]["widgets"]
        .as_array()
        .expect("widgets is a list");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["name"], serde_json::Value::Null);
    }
}

#[tokio::test]
async fn irreconcilable_masked_value_fails_closed() {
    let app = boot().await;
    let json = query(&app, "viewer", "{ strictWidget { id name } }").await;
    assert!(
        !json["errors"].as_array().unwrap_or(&vec![]).is_empty(),
        "masking away a required field must surface an error, not data"
    );
    assert_eq!(
        json["data"], serde_json::Value::Null,
        "no partial unmasked data may ship"
    );
}

#[tokio::test]
async fn zero_grant_caller_is_gated_before_masking() {
    let app = boot().await;
    let json = query(&app, "", "{ widget { id name } }").await;
    assert!(
        !json["errors"].as_array().unwrap_or(&vec![]).is_empty(),
        "the emitted class gate rejects a caller with no Read rule"
    );
}

#[tokio::test]
async fn public_posture_skips_gate_and_mask() {
    let app = boot().await;
    let json = query(&app, "", "{ motd }").await;
    assert_eq!(json["data"]["motd"], "hello");
}
