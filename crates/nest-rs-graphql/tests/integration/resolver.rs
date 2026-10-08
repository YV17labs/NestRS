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

// Linked into this binary, but unreachable here.
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

// `#[field_resolver]`'s position 1 is the **parent**, so `&Context` comes second.

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

    /// Parent first, then the context: the context is the wrapper's `__ctx`, not
    /// an injected dependency.
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

// Method syntax resolves on the receiver before it derefs: a root holds its
// resolver in an `Arc`, a field resolver builds it by value.

#[expect(
    dead_code,
    reason = "never called: the expansion calls the method by its path"
)]
trait ShadowsOnArc {
    fn crate_label(&self) -> String;
}

impl<T> ShadowsOnArc for std::sync::Arc<T> {
    fn crate_label(&self) -> String {
        "the trait on Arc".into()
    }
}

#[expect(
    dead_code,
    reason = "never called: the expansion calls the method by its path"
)]
trait ShadowsByValue {
    fn weight(self, parent: &Crate) -> nest_rs_graphql::async_graphql::Result<String>;
}

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Crate {
    id: i32,
}

#[resolver]
struct CrateResolver;

impl ShadowsByValue for CrateResolver {
    fn weight(self, _parent: &Crate) -> nest_rs_graphql::async_graphql::Result<String> {
        Ok("the trait by value".into())
    }
}

#[operations]
impl CrateResolver {
    #[query]
    #[public]
    fn crate_label(&self) -> String {
        "the operation".into()
    }

    #[query]
    #[public]
    async fn shipment(&self) -> Crate {
        Crate { id: 1 }
    }

    #[field_resolver]
    fn weight(&self, parent: &Crate) -> nest_rs_graphql::async_graphql::Result<String> {
        Ok(format!("the field resolver {}", parent.id))
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [CrateResolver])]
struct CrateModule;

#[tokio::test]
async fn an_operation_calls_its_method_and_not_a_trait_method_of_the_same_name() {
    let app = TestApp::for_module::<CrateModule>()
        .await
        .expect("the schema boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ crateLabel shipment { weight } }" }))
        .send()
        .await;
    let body = resp.json().await.value().deserialize::<serde_json::Value>();

    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["crateLabel"], "the operation", "{body}");
    assert_eq!(
        body["data"]["shipment"]["weight"], "the field resolver 1",
        "{body}"
    );
}

// async-graphql's camel case reads `a1_b` and `a_1b` as one field; the compile-time
// refusal is `nest-rs-macro-hygiene`'s `operations_two_methods_one_served_name`.

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Ledger {
    id: i32,
}

#[resolver]
struct NamingResolver;

#[operations]
impl NamingResolver {
    #[query]
    #[public]
    async fn get_2fa(&self) -> String {
        "get_2fa".into()
    }

    #[query]
    #[public]
    async fn user_id(&self, snake: i32) -> String {
        format!("user_id:{snake}")
    }

    #[query]
    #[public]
    #[expect(
        non_snake_case,
        reason = "the field's spelled case is the shape under test"
    )]
    async fn userID(&self, upper: i32) -> String {
        format!("userID:{upper}")
    }

    #[query]
    #[public]
    async fn ledger(&self, id: i32) -> Ledger {
        Ledger { id }
    }

    #[field_resolver]
    async fn a_1b_total(&self, parent: &Ledger, right: i32) -> String {
        format!("a_1b_total:{}:{right}", parent.id)
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [NamingResolver])]
struct NamingModule;

async fn naming_query(query: &str) -> serde_json::Value {
    let app = TestApp::for_module::<NamingModule>()
        .await
        .expect("the schema boots");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    resp.json().await.value().deserialize::<serde_json::Value>()
}

#[tokio::test]
async fn a_digit_after_an_underscore_starts_a_word_as_async_graphql_reads_it() {
    let body = naming_query("{ get2Fa }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["get2Fa"], "get_2fa", "{body}");
}

#[tokio::test]
async fn a_capitalised_method_name_is_its_own_field() {
    let body = naming_query("{ userId(snake: 1) userID(upper: 2) }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["userId"], "user_id:1", "{body}");
    assert_eq!(body["data"]["userID"], "userID:2", "{body}");
}

#[tokio::test]
async fn a_field_resolver_is_named_by_the_same_rule() {
    let body = naming_query("{ ledger(id: 4) { a1BTotal(right: 2) } }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(
        body["data"]["ledger"]["a1BTotal"], "a_1b_total:4:2",
        "{body}"
    );
}

// The name `#[operations]` states must be the one async-graphql's own derive
// serves; the oracle is that derive, never a copy of its rule.

use nest_rs_graphql::async_graphql;

/// The method names both halves carry: a digit after `_`, a digit run, a digit
/// before a letter, a capital run, and the plain case.
struct Oracle;

#[async_graphql::Object]
impl Oracle {
    async fn get_2fa(&self) -> i32 {
        1
    }
    async fn a_1b(&self) -> i32 {
        1
    }
    async fn page_2_items(&self) -> i32 {
        1
    }
    async fn v2_api(&self) -> i32 {
        1
    }
    #[expect(
        non_snake_case,
        reason = "the field's spelled case is the shape under test"
    )]
    async fn userID(&self) -> i32 {
        1
    }
    async fn user_count(&self) -> i32 {
        1
    }
}

#[resolver]
struct OracleResolver;

#[operations]
impl OracleResolver {
    #[query]
    #[public]
    async fn get_2fa(&self) -> i32 {
        1
    }
    #[query]
    #[public]
    async fn a_1b(&self) -> i32 {
        1
    }
    #[query]
    #[public]
    async fn page_2_items(&self) -> i32 {
        1
    }
    #[query]
    #[public]
    async fn v2_api(&self) -> i32 {
        1
    }
    #[expect(
        non_snake_case,
        reason = "the field's spelled case is the shape under test"
    )]
    #[query]
    #[public]
    async fn userID(&self) -> i32 {
        1
    }
    #[query]
    #[public]
    async fn user_count(&self) -> i32 {
        1
    }
}

#[module(providers = [OracleResolver])]
struct OracleFeatureModule;

#[module(imports = [
    GraphqlModule::for_root(Some(GraphqlConfig {
        disable_introspection: false,
        ..GraphqlConfig::default()
    })),
    OracleFeatureModule,
])]
struct AppWithOracleNames;

const QUERY_FIELDS: &str = "{ __schema { queryType { fields { name } } } }";

fn field_names(data: &serde_json::Value) -> std::collections::BTreeSet<String> {
    data["__schema"]["queryType"]["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|field| field["name"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_field_is_served_under_the_name_async_graphql_gives_the_method() {
    let oracle = async_graphql::Schema::build(
        Oracle,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .finish()
    .execute(QUERY_FIELDS)
    .await;
    assert!(oracle.errors.is_empty(), "{:?}", oracle.errors);
    let expected = field_names(&oracle.data.into_json().expect("introspection answers JSON"));
    assert!(
        expected.contains("get2Fa") && expected.contains("a1B"),
        "the oracle splits at a digit: {expected:?}",
    );

    let app = TestApp::builder()
        .module::<AppWithOracleNames>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": QUERY_FIELDS }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let text = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a GraphQL response body");
    let body: serde_json::Value = serde_json::from_str(&text).expect("a GraphQL response is JSON");
    let served = field_names(&body["data"]);

    let missing: Vec<&String> = expected.difference(&served).collect();
    assert!(
        missing.is_empty(),
        "fields async-graphql would serve that #[operations] does not: {missing:?} — served \
         {served:?}",
    );
}
