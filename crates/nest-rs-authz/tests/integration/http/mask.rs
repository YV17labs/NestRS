//! Wire → model → mask → retain wire keys, driven through a real `#[routes]`
//! handler and the `Authorize` response shaper — no live database.

use std::sync::Arc;

use nest_rs_authz::http::Authorize;
use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::poem::web::Json;
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_resource::WireModelDefaults;
use nest_rs_testing::TestApp;
use poem::Request;
use serde::Serialize;

mod widget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(
        Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize, schemars::JsonSchema,
    )]
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

// `JsonSchema` too: an `Authorize`-shaped route publishes its response schema
// (OAPI-O5), so the bound `Json<T>` has always carried on an unshaped route now
// applies uniformly — adding `#[authorize]` no longer silently drops a route's
// documented shape.
#[derive(Serialize, schemars::JsonSchema)]
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
        req.extensions_mut()
            .insert(Arc::new(b.build().expect("valid test ability")));
        Ok(())
    }
}

impl HttpGuard for AbilityInjector {}

#[injectable]
#[derive(Default)]
struct ListAbilityInjector;

impl Layer for ListAbilityInjector {}

#[async_trait]
impl Guard for ListAbilityInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        req.extensions_mut()
            .insert(Arc::new(b.build().expect("valid test ability")));
        Ok(())
    }
}

impl HttpGuard for ListAbilityInjector {}

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

    // The decorator form: `#[authorize]` must arm exactly what the hand-written
    // `Authorize<..>` parameter above arms — same gate, same response shaper —
    // with nothing in the signature to delete by accident.
    #[get("/decorated")]
    #[authorize(Read, widget::Entity)]
    async fn decorated(&self) -> Json<widget::Model> {
        Json(widget::Model {
            id: 1,
            name: "ada".into(),
            secret: "s1".into(),
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

    /// RFC 6839: `+json` is a structured syntax suffix, so this **is** JSON.
    #[get("/vendor")]
    async fn vendor(&self, _authz: Authorize<Read, widget::Entity>) -> poem::Response {
        raw_widget_response(Some("application/vnd.api+json"))
    }

    /// RFC 9110 §8.3.1: type and subtype are case-insensitive, so this is the
    /// *same* media type as `application/json`.
    #[get("/upper")]
    async fn upper(&self, _authz: Authorize<Read, widget::Entity>) -> poem::Response {
        raw_widget_response(Some("Application/JSON"))
    }

    /// Parameters are not part of the media type.
    #[get("/charset")]
    async fn charset(&self, _authz: Authorize<Read, widget::Entity>) -> poem::Response {
        raw_widget_response(Some("application/json; charset=utf-8"))
    }

    /// Nothing declared at all — the shaper cannot classify it, so it must not
    /// guess in the direction that ships the body.
    #[get("/untyped")]
    async fn untyped(&self, _authz: Authorize<Read, widget::Entity>) -> poem::Response {
        raw_widget_response(None)
    }

    /// A declared non-JSON type still passes through untouched: the handler said
    /// what it was returning, and there is no entity in it to mask.
    #[get("/csv")]
    async fn csv(&self, _authz: Authorize<Read, widget::Entity>) -> poem::Response {
        poem::Response::builder()
            .content_type("text/csv")
            .body("id,name\n1,ada\n")
    }
}

/// A hand-built response carrying the unexposed `secret`, under whichever
/// `Content-Type` spelling the caller asks for — or none at all.
fn raw_widget_response(content_type: Option<&str>) -> poem::Response {
    let body = r#"{"id":1,"name":"ada","secret":"s1"}"#;
    let mut builder = poem::Response::builder();
    if let Some(content_type) = content_type {
        builder = builder.content_type(content_type);
    }
    builder.body(body)
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
async fn the_authorize_decorator_arms_the_same_gate_and_mask() {
    // API-1: `#[authorize(Read, Entity)]` with an empty signature must behave
    // exactly like the hand-written `Authorize<..>` parameter — masked on a
    // grant, 403 without one — so posture never depends on how an import was
    // spelled.
    let app = boot().await;

    let resp = app
        .http()
        .get("/widgets/decorated")
        .header("x-role", "admin")
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("secret") && !body.contains("s1"),
        "the decorator must arm the response shaper: {body}",
    );

    // …and the field restriction of a non-admin grant applies just the same.
    let restricted = app.http().get("/widgets/decorated").send().await;
    restricted.assert_status_is_ok();
    let body = restricted.0.into_body().into_string().await.expect("body");
    let json: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(json.get("name").and_then(|v| v.as_str()), Some("ada"));
    assert!(
        json.get("id").is_none() && json.get("secret").is_none(),
        "a restricted grant masks down to its permitted fields: {body}",
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

/// OAPI-O5: an ability shaper masks *fields*, so a route behind one publishes
/// its full shape and flags it — it does not publish nothing.
///
/// Suppressing the schema was the honest reading of "the field set depends on
/// the caller", and it typed every `#[crud]` response as `any` in a generated
/// client, on exactly the surface `#[expose]` exists to serve. Asserted on the
/// route metadata rather than on a rendered document so the contract is pinned
/// where it is produced — `#[routes]` — and with no database in reach.
#[tokio::test]
async fn a_shaped_route_still_records_its_response_schema_and_says_it_is_masked() {
    let app = boot().await;
    let discovery = nest_rs_core::Discovery::new(app.container());
    let controllers = discovery.meta::<nest_rs_http::HttpControllerMeta>();
    let routes: Vec<_> = controllers
        .iter()
        .flat_map(|d| d.meta.routes.iter())
        .collect();
    assert!(!routes.is_empty(), "the controller is discovered at all");

    // The media-type probes return a bare `poem::Response` precisely so they can
    // set (or omit) a `Content-Type` no typed body would let them set. An opaque
    // return type has no schema to publish, so they answer the `masked` half of
    // this contract and are exempt from the `response` half — by name, so a route
    // cannot drift out of the assertion by accident.
    const OPAQUE_BY_CONSTRUCTION: &[&str] = &["vendor", "upper", "charset", "untyped", "csv"];

    for route in routes {
        assert!(
            route.masked,
            "every route here is shaped, so every one is masked: {}",
            route.handler,
        );
        if OPAQUE_BY_CONSTRUCTION.contains(&route.handler) {
            continue;
        }
        assert!(
            route.response.is_some(),
            "a masked route publishes the shape a caller may see a subset of: {}",
            route.handler,
        );
    }
}

/// The mask is armed by the compiler and cannot be renamed out of — but what
/// arming installs then decided *whether to run* by comparing the response's
/// `Content-Type` against the literal prefix `application/json`. Three bodies
/// that are JSON by the standard failed that test and shipped every unexposed
/// column, at `200`, with the route armed and nothing logged.
///
/// Each case below is one of them, and each is a media type the standard says is
/// JSON — not a near-miss.
#[tokio::test]
async fn a_json_media_type_is_masked_however_it_is_spelled() {
    let app = boot().await;
    for path in ["/widgets/vendor", "/widgets/upper", "/widgets/charset"] {
        let resp = app.http().get(path).header("x-role", "admin").send().await;
        resp.assert_status_is_ok();
        let body = resp.0.into_body().into_string().await.expect("body");
        assert!(
            !body.contains("secret") && !body.contains("s1"),
            "{path} is a JSON media type, so the mask must run: {body}",
        );
        assert!(
            body.contains("ada"),
            "and the exposed columns survive: {body}",
        );
    }
}

/// A response that declares nothing cannot be classified, so the shaper must not
/// guess. Passing it through is the only guess that leaks, which makes failing
/// closed the answer here — the same one a body that will not reconcile with the
/// entity already gets.
#[tokio::test]
async fn a_body_with_no_declared_media_type_fails_closed() {
    // Thread-local: `#[tokio::test]` is a current-thread runtime, so the
    // route's task runs on this thread.
    let logs = nest_rs_testing::LogCapture::install();
    let app = boot().await;
    let resp = app
        .http()
        .get("/widgets/untyped")
        .header("x-role", "admin")
        .send()
        .await;
    resp.assert_status(poem::http::StatusCode::INTERNAL_SERVER_ERROR);
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("secret") && !body.contains("s1"),
        "and it ships none of the entity on the way out: {body}",
    );

    // The 500 is the same 500 a panicking handler gives, so the event is what
    // separates "this route is broken" from "this route was refused a body it
    // could not classify". Every fail-closed masking exit files this one line —
    // which is what makes a branch that forgets it the visible omission — and
    // nothing read it until a refusal that was never a masking failure stopped
    // standing in for it.
    let event = logs.expect_one("nest_rs::authz", "response masking failed");
    assert_eq!(event.level, "warn");
    assert!(
        event.field("entity").is_some_and(|e| e.contains("widget")),
        "the event names the entity the shaper was armed for, got {:?}",
        event.fields,
    );
    assert!(
        event.field("reason").is_some(),
        "…and which step of the mask refused, got {:?}",
        event.fields,
    );
}

/// The converse, so the rule above is a classification and not a ban: a handler
/// that *names* a non-JSON type has said what it is returning, and there is no
/// entity in it to mask.
#[tokio::test]
async fn a_declared_non_json_body_still_passes_through() {
    let app = boot().await;
    let resp = app
        .http()
        .get("/widgets/csv")
        .header("x-role", "admin")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("id,name\n1,ada\n").await;
}
