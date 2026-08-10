//! Automatic response masking through the real macros: `#[authorize(Action,
//! Entity)]` on a `#[query]` makes `#[resolver]` emit `masked_value_for`
//! around the return value — the resolver body writes no masking call. Covers
//! the wire shapes the wrapper sees through (bare DTO, `Vec`), the fail-closed
//! path (a required field the mask strips ⇒ GraphQL error, never unmasked
//! data), and the `#[public]` opt-out.

use std::sync::Arc;

use nest_rs_authz::graphql::GraphqlAbilityBridge;
use nest_rs_authz::{AbilityBuilder, Action, MaskReplyError, Read, masked_reply, with_ability};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::{Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlModule, GraphqlOperationGuard, operations, resolver};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::async_trait;
use nest_rs_http::poem::Request;
use nest_rs_resource::WireModelDefaults;
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};

use super::query;

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
/// unrestricted; `viewer` reads widgets but only the `id` field; `auditor`
/// reads only widget 1 and only its `id`; anyone else gets nothing.
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
            "auditor" => {
                b.can(Action::Read, widget::Entity)
                    .when(|p| p.eq(widget::Column::Id, 1))
                    .fields([widget::Column::Id]);
            }
            _ => {}
        }
        req.extensions_mut()
            .insert(Arc::new(b.build().expect("valid test ability")));
        Ok(())
    }
}

#[resolver]
struct MaskResolver;

#[operations]
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
    #[authorize(Read, widget::Entity)]
    async fn strict_widgets(&self) -> GqlResult<Vec<StrictWidgetDto>> {
        Ok(vec![
            StrictWidgetDto {
                id: 1,
                name: "ada".into(),
            },
            StrictWidgetDto {
                id: 2,
                name: "grace".into(),
            },
        ])
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

// ── A field grant against a non-null schema field ───────────────────────────
//
// `#[expose]` types a non-nullable column as a non-null GraphQL field, which
// the mask cannot null. The selection set decides instead: asking for the
// stripped field is refused, asking for the granted ones is served.

#[tokio::test]
async fn selecting_a_field_the_grant_strips_is_refused() {
    let app = boot().await;
    let json = query(&app, "viewer", "{ strictWidget { id name } }").await;
    assert_eq!(
        json["data"],
        serde_json::Value::Null,
        "no partial unmasked data may ship: {json}",
    );
    assert_eq!(
        json["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "a field outside the grant is a denial, not a masking failure: {json}",
    );
    // D3: a **list**, not a comma-joined string — the natural reading of
    // "names in the `fields` extension", and the only shape that survives more
    // than one refused field without every client re-splitting it.
    assert_eq!(
        json["errors"][0]["extensions"]["fields"],
        serde_json::json!(["name"]),
        "the denial names the field it refused, as a list: {json}",
    );
}

#[tokio::test]
async fn selecting_only_granted_fields_still_serves_the_entity() {
    let app = boot().await;
    let json = query(&app, "viewer", "{ strictWidget { id } }").await;
    assert!(
        json.get("errors").is_none(),
        "a query asking only for granted columns must succeed: {json}",
    );
    assert_eq!(json["data"]["strictWidget"]["id"], 1);
}

#[tokio::test]
async fn an_unrestricted_caller_reads_every_field_of_a_strict_shape() {
    let app = boot().await;
    let json = query(&app, "admin", "{ strictWidget { id name } }").await;
    assert_eq!(json["data"]["strictWidget"]["name"], "ada", "{json}");
}

#[tokio::test]
async fn rows_the_ability_refuses_are_still_dropped_from_a_strict_list() {
    let app = boot().await;
    let json = query(&app, "auditor", "{ strictWidgets { id } }").await;
    let rows = json["data"]["strictWidgets"]
        .as_array()
        .unwrap_or_else(|| panic!("strictWidgets is a list: {json}"));
    assert_eq!(rows.len(), 1, "widget 2 is outside the grant: {json}");
    assert_eq!(rows[0]["id"], 1);
}

#[tokio::test]
async fn a_caller_with_no_grant_reads_no_strict_row() {
    let app = boot().await;
    let json = query(&app, "", "{ strictWidgets { id } }").await;
    assert_eq!(
        json["data"],
        serde_json::Value::Null,
        "the class gate rejects a caller with no Read rule: {json}",
    );
    assert!(!json["errors"].as_array().unwrap_or(&vec![]).is_empty());
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

// ── SEC-F1: `masked_reply` — the manual opt-in helper WS gateways must call ──

#[tokio::test]
async fn masked_reply_fails_closed_without_an_ambient_ability() {
    // With no ambient ability installed (the transport's authz bridge missing
    // or a handler that forgot to run inside it) the helper must error, never
    // pass the unmasked wire value through.
    let wire = serde_json::json!({ "id": 1, "name": "ada" });
    let result = masked_reply::<widget::Entity>(Action::Read, wire);
    assert!(
        matches!(result, Err(MaskReplyError::NoAmbientAbility)),
        "no ambient ability must fail closed, not passthrough",
    );
}

#[tokio::test]
async fn masked_reply_strips_unpermitted_fields_and_unexposed_columns() {
    // A viewer may read only the `id` field: `name` is masked to null and the
    // server-only `secret` column is strained out — the same semantics the HTTP
    // shaper and GraphQL wrapper apply automatically.
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .fields([widget::Column::Id]);
    let ability = Arc::new(b.build().expect("valid ability"));
    let wire = serde_json::json!({ "id": 1, "name": "ada" });

    let masked = with_ability(ability, async move {
        masked_reply::<widget::Entity>(Action::Read, wire)
    })
    .await
    .expect("masks with an ambient ability");

    assert_eq!(masked["id"], 1);
    assert_eq!(
        masked["name"],
        serde_json::Value::Null,
        "a field outside the grant is masked",
    );
    assert!(
        masked.get("secret").is_none(),
        "the server-only column is strained out",
    );
}

#[tokio::test]
async fn masked_reply_masks_every_row_of_an_array() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .fields([widget::Column::Id]);
    let ability = Arc::new(b.build().expect("valid ability"));
    let wire = serde_json::json!([
        { "id": 1, "name": "ada" },
        { "id": 2, "name": "grace" },
    ]);

    let masked = with_ability(ability, async move {
        masked_reply::<widget::Entity>(Action::Read, wire)
    })
    .await
    .expect("masks an array");

    let rows = masked.as_array().expect("array reply");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["name"], serde_json::Value::Null);
        assert!(row.get("secret").is_none());
    }
}
