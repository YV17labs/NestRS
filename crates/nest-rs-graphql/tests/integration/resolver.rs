//! Module-gating: a resolver in a reachable module appears in the schema; a
//! resolver in no reachable module is silently skipped.

use nest_rs_core::module;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_testing::TestApp;

#[resolver]
struct LooseResolver;

#[operations]
impl LooseResolver {
    #[query]
    #[public]
    async fn loose(&self) -> String {
        "ok".into()
    }
}

#[module(providers = [LooseResolver])]
struct LooseFeatureModule;

#[module(imports = [GraphqlModule::for_root(None), LooseFeatureModule])]
struct AppWithLoose;

// The resolver is linked (the inventory is shared with the other test in
// this binary) but unreachable here — module-gating must skip it.
#[module(imports = [GraphqlModule::for_root(Some(GraphqlConfig {
    disable_introspection: false,
    ..GraphqlConfig::default()
}))])]
struct AppWithoutLoose;

#[tokio::test]
async fn a_reachable_resolver_appears_in_the_schema() {
    let app = TestApp::builder()
        .module::<AppWithLoose>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ loose }" }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let loose = json
        .value()
        .object()
        .get("data")
        .object()
        .get("loose")
        .string();
    assert_eq!(loose, "ok");
}

#[tokio::test]
async fn an_unreachable_resolver_is_filtered_from_the_schema() {
    let app = TestApp::builder()
        .module::<AppWithoutLoose>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("an app composes only the resolvers in its reachable modules");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "{ __type(name: \"Query\") { fields { name } } }"
        }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let fields = json
        .value()
        .object()
        .get("data")
        .object()
        .get("__type")
        .object()
        .get("fields")
        .array();
    for field in fields.iter() {
        let name = field.object().get("name").string();
        assert_ne!(name, "loose", "unreachable resolver leaked into the schema",);
    }
}

// ---------------------------------------------------------------------------
// `#[field_resolver]`'s parameter shape. Its position 1 is the **parent**, so a
// `&Context` correctly comes second — the one operation role where that is true.

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Parcel {
    id: i32,
}

#[resolver]
struct ParcelResolver;

#[operations]
impl ParcelResolver {
    #[query]
    #[public]
    async fn parcel(&self, id: i32) -> Parcel {
        Parcel { id }
    }

    /// The shape the docs teach: parent first, then the context. It compiled and
    /// then answered `no provider registered for `& Context < '_ >`` on every
    /// request — the context fell through to the injected-dep arm and was asked
    /// of the container. Now it is the `__ctx` the wrapper already holds.
    #[field_resolver]
    async fn tag(
        &self,
        parent: &Parcel,
        ctx: &nest_rs_graphql::async_graphql::Context<'_>,
    ) -> nest_rs_graphql::async_graphql::Result<String> {
        let _ = ctx.data_opt::<nest_rs_core::Container>();
        Ok(format!("parcel-{}", parent.id))
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [ParcelResolver])]
struct ParcelModule;

#[tokio::test]
async fn a_field_resolver_takes_the_context_after_its_parent() {
    let app = TestApp::for_module::<ParcelModule>()
        .await
        .expect("the schema boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ parcel(id: 3) { id tag } }" }))
        .send()
        .await;
    let body = resp.json().await.value().deserialize::<serde_json::Value>();

    assert!(
        body["errors"].is_null(),
        "the documented shape resolves rather than reporting a missing provider: {body}",
    );
    assert_eq!(body["data"]["parcel"]["tag"], "parcel-3", "{body}");
}
