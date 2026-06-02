//! A `#[resolver]` self-composes into the GraphQL schema through the link-time
//! registry. Module-gating filters that registry at schema-build time by the
//! reachable provider set: a resolver listed in `providers = [...]` of a
//! reachable module appears in the schema; a resolver in no reachable module
//! is silently skipped (a `tracing::warn` surfaces it at boot so leftover
//! code does not disappear without trace).
//!
//! The two cases below share one `LooseResolver` and differ by whether the
//! root module imports the resolver-owning module. When imported, the
//! resolver's `loose` query lands in the schema; when not, it is filtered
//! out and the app boots cleanly.

use nestrs_core::module;
use nestrs_graphql::{resolver, GraphqlModule};
use nestrs_http::HttpTransport;
use nestrs_testing::TestApp;

#[resolver]
struct LooseResolver;

#[resolver]
impl LooseResolver {
    #[query]
    async fn loose(&self) -> String {
        "ok".into()
    }
}

// Lists `LooseResolver` as a provider — importing this module makes the
// resolver reachable, so its `loose` query lands in the schema.
#[module(providers = [LooseResolver])]
struct LooseFeatureModule;

// Root that imports `LooseFeatureModule` — the resolver is reachable.
#[module(imports = [GraphqlModule::for_root(None), LooseFeatureModule])]
struct AppWithLoose;

// Root that does NOT import `LooseFeatureModule` — the resolver is linked
// (any test in this binary that uses `#[resolver] struct LooseResolver`
// shares the same inventory) but unreachable, so module-gating must skip it.
#[module(imports = [GraphqlModule::for_root(None)])]
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
    // Boot succeeds even though `LooseResolver` is linked: module-gating
    // skips it from the schema rather than failing the boot.
    let app = TestApp::builder()
        .module::<AppWithoutLoose>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("an app composes only the resolvers in its reachable modules");

    // The schema does not advertise the unreachable resolver — an
    // introspection of the root Query returns no `loose` field.
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
        assert_ne!(
            name, "loose",
            "unreachable resolver leaked into the schema",
        );
    }
}
