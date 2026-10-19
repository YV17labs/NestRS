//! Wire → model → mask → retain wire keys, driven through a real `#[routes]`
//! handler and the `Authorize` response shaper — no live database.

use nest_rs_authz::http::Authorize;
use nest_rs_authz::{AbilityBuilder, Action, Read, WireModelDefaults};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::Json;
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_testing::TestApp;
use serde::Serialize;

mod widget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(
        Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize, schemars::JsonSchema,
    )]
    #[sea_orm(table_name = "widgets")]
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

    fn is_placeholdered(column: widget::Column) -> bool {
        matches!(column, widget::Column::Secret)
    }
}

// `JsonSchema` too: an `Authorize`-shaped route publishes its response schema.
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
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let admin = nest_rs_http::HttpView::of(cx)?
            .request()
            .header("x-role")
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
        cx.attach(b.build().expect("valid test ability"));
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AbilityInjector {}

#[injectable]
#[derive(Default)]
struct ListAbilityInjector;

impl Layer for ListAbilityInjector {}

#[async_trait]
impl Guard for ListAbilityInjector {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        cx.attach(b.build().expect("valid test ability"));
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for ListAbilityInjector {}

#[controller(path = "/widgets")]
struct WidgetController;

#[routes]
impl WidgetController {
    #[get("/{id}")]
    async fn one(&self, _authz: Authorize<Read, widget::Entity>) -> Json<WidgetDto> {
        Json(WidgetDto {
            id: 1,
            name: "ada".into(),
        })
    }

    // The decorator form arms exactly what the hand-written `Authorize<..>` parameter arms.
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
    async fn vendor(&self, _authz: Authorize<Read, widget::Entity>) -> nest_rs_http::Response {
        raw_widget_response(Some("application/vnd.api+json"))
    }

    /// RFC 9110 §8.3.1: type and subtype are case-insensitive, so this is the
    /// *same* media type as `application/json`.
    #[get("/upper")]
    async fn upper(&self, _authz: Authorize<Read, widget::Entity>) -> nest_rs_http::Response {
        raw_widget_response(Some("Application/JSON"))
    }

    /// Parameters are not part of the media type.
    #[get("/charset")]
    async fn charset(&self, _authz: Authorize<Read, widget::Entity>) -> nest_rs_http::Response {
        raw_widget_response(Some("application/json; charset=utf-8"))
    }

    /// Nothing declared at all — the shaper cannot classify it, so it must not
    /// guess in the direction that ships the body.
    #[get("/untyped")]
    async fn untyped(&self, _authz: Authorize<Read, widget::Entity>) -> nest_rs_http::Response {
        raw_widget_response(None)
    }

    /// A declared non-JSON type still passes through untouched: the handler said
    /// what it was returning, and there is no entity in it to mask.
    #[get("/csv")]
    async fn csv(&self, _authz: Authorize<Read, widget::Entity>) -> nest_rs_http::Response {
        nest_rs_http::Response::builder()
            .content_type(nest_rs_http::HeaderValue::from_static("text/csv"))
            .body("id,name\n1,ada\n")
    }
}

/// A hand-built response carrying the unexposed `secret`, under whichever
/// `Content-Type` spelling the caller asks for — or none at all.
fn raw_widget_response(content_type: Option<&'static str>) -> nest_rs_http::Response {
    let body = r#"{"id":1,"name":"ada","secret":"s1"}"#;
    let mut builder = nest_rs_http::Response::builder();
    if let Some(content_type) = content_type {
        builder = builder.content_type(nest_rs_http::HeaderValue::from_static(content_type));
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
    // A handler returning `Json(Model)` instead of the wire DTO must not leak the
    // unexposed `secret`, even under an unrestricted (admin) grant.
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
    // `#[authorize(Read, Entity)]` with an empty signature behaves exactly like
    // the hand-written `Authorize<..>` parameter.
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
    // When `mask_many` drops a row (id=2 denied) under an unrestricted grant on
    // id=1, the survivor is still strained.
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

/// `narrow` holds a title-only grant beside a relational one; anyone else reads
/// every post but those a relation denies.
#[injectable]
#[derive(Default)]
struct PostAbilityInjector;

impl Layer for PostAbilityInjector {}

#[async_trait]
impl Guard for PostAbilityInjector {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        use crate::post;

        let narrow = nest_rs_http::HttpView::of(cx)?
            .request()
            .header("x-role")
            .is_some_and(|role| role == "narrow");
        let mut b = AbilityBuilder::new();
        if narrow {
            b.can(Action::Read, post::Entity)
                .fields([post::Column::Title]);
            b.can(Action::Read, post::Entity)
                .when(|p| post::in_org(&p, 7));
        } else {
            b.can(Action::Read, post::Entity);
            b.cannot(Action::Read, post::Entity)
                .when(|p| post::in_org(&p, 9));
        }
        cx.attach(b.build().expect("valid test ability"));
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for PostAbilityInjector {}

#[controller(path = "/posts")]
struct PostController;

#[routes]
impl PostController {
    /// The rows a `Repo` read under the caller's ability returns: its SQL
    /// applied every rule, the relational ones included.
    #[get("/")]
    #[authorize(Read, crate::post::Entity)]
    async fn list(&self) -> Json<Vec<crate::post::Post>> {
        let row = |id: i32| {
            crate::post::Post::from(&crate::post::Model {
                id,
                org_id: 7,
                title: format!("post {id}"),
                archived_at: None,
                deleted: false,
            })
        };
        Json(vec![row(1), row(2)])
    }
}

#[module(providers = [PostAbilityInjector, PostController])]
struct PostModule;

async fn posts(role: &str) -> serde_json::Value {
    let app = TestApp::builder()
        .module::<PostModule>()
        .use_guards_global([guard::<PostAbilityInjector>()])
        .build()
        .await
        .expect("post harness boots");
    let resp = app.http().get("/posts").header("x-role", role).send().await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    serde_json::from_str(&body).expect("a JSON body")
}

/// The denial's relation was decided in SQL, which excluded what it denies:
/// re-deciding it on a rebuilt row would drop every row SQL admitted.
#[tokio::test]
async fn a_relational_denial_keeps_every_row_the_read_admitted() {
    let rows = posts("member").await;
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(2),
        "the read already left out what the denial names: {rows}",
    );
}

/// A row the title-only grant decides takes that grant's fields; the
/// relational grant cannot prove it matched this row, so it widens nothing.
#[tokio::test]
async fn a_relational_grant_widens_no_row_a_decided_grant_narrows() {
    let rows = posts("narrow").await;
    let rows = rows.as_array().expect("a list");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(
            row.as_object().map(|o| o.keys().collect::<Vec<_>>()),
            Some(vec![&"title".to_owned()]),
            "{row}",
        );
    }
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

/// An ability shaper masks *fields*, so a route behind one publishes its full
/// shape and flags it — it does not publish nothing.
#[tokio::test]
async fn a_shaped_route_still_records_its_response_schema_and_says_it_is_masked() {
    let app = boot().await;
    let discovery = nest_rs_core::Discovery::new(app.container());
    let controllers = discovery.meta::<nest_rs_http::HttpControllerMeta>();
    let routes: Vec<_> = controllers
        .iter()
        .flat_map(|d| d.meta.routes().iter())
        .collect();
    assert!(!routes.is_empty(), "the controller is discovered at all");

    // The media-type probes return a bare `poem::Response` to set (or omit) a
    // `Content-Type`; with no schema to publish they are exempt from the
    // `response` half, by name.
    const OPAQUE_BY_CONSTRUCTION: &[&str] = &["vendor", "upper", "charset", "untyped", "csv"];

    for route in routes {
        assert!(
            route.masked(),
            "every route here is shaped, so every one is masked: {}",
            route.handler(),
        );
        if OPAQUE_BY_CONSTRUCTION.contains(&route.handler()) {
            continue;
        }
        assert!(
            route.response().is_some(),
            "a masked route publishes the shape a caller may see a subset of: {}",
            route.handler(),
        );
    }
}

/// Each case is a media type the standard says is JSON — not a near-miss — so
/// the armed mask must run on it.
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

/// A response that declares nothing cannot be classified, so the shaper fails
/// closed rather than guess.
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
    resp.assert_status(nest_rs_http::StatusCode::INTERNAL_SERVER_ERROR);
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("secret") && !body.contains("s1"),
        "and it ships none of the entity on the way out: {body}",
    );

    // The 500 matches a panicking handler's; the event is what tells them apart.
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
