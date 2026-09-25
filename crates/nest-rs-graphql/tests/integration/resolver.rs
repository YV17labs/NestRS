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

// ---------------------------------------------------------------------------
// The call reaches the developer's method, whatever else shares its name. Method
// syntax resolves on the receiver *before* it derefs: an operation's root holds
// its resolver in an `Arc`, so a trait implemented for `Arc<T>` answered first,
// and a field resolver builds its resolver by value, so a trait taking `self`
// for `T` did.

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

// ---------------------------------------------------------------------------
// The name a field is served under is the name its identity was checked under.
// async-graphql's own rule (`Inflector`'s camel case) read `a1_b` and `a_1b` as
// one field, and `user_id` and `userID` as one, while `#[operations]` read them
// as two — so both compiled, and the schema documented one method's arguments
// while the other's body ran. Every field is now named by the framework, so the
// check and the schema read one rule.

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
    async fn a1_b(&self, left: i32) -> String {
        format!("a1_b:{left}")
    }

    #[query]
    #[public]
    async fn a_1b(&self, right: i32) -> String {
        format!("a_1b:{right}")
    }

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
    #[allow(non_snake_case)]
    async fn userID(&self, upper: i32) -> String {
        format!("userID:{upper}")
    }

    #[query]
    #[public]
    async fn ledger(&self, id: i32) -> Ledger {
        Ledger { id }
    }

    #[field_resolver]
    async fn a1_b_total(&self, parent: &Ledger, left: i32) -> String {
        format!("a1_b_total:{}:{left}", parent.id)
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
async fn two_methods_the_camel_case_rule_folds_alike_are_two_fields_each_running_its_own_body() {
    let body = naming_query("{ a1B(left: 1) a1b(right: 2) }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["a1B"], "a1_b:1", "{body}");
    assert_eq!(body["data"]["a1b"], "a_1b:2", "{body}");
}

#[tokio::test]
async fn a_digit_after_an_underscore_is_served_as_written() {
    let body = naming_query("{ get2fa }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["get2fa"], "get_2fa", "{body}");
}

#[tokio::test]
async fn a_capitalised_method_name_is_its_own_field() {
    let body = naming_query("{ userId(snake: 1) userID(upper: 2) }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["userId"], "user_id:1", "{body}");
    assert_eq!(body["data"]["userID"], "userID:2", "{body}");
}

#[tokio::test]
async fn field_resolvers_the_camel_case_rule_folds_alike_each_run_their_own_body() {
    let body = naming_query("{ ledger(id: 4) { a1BTotal(left: 1) a1bTotal(right: 2) } }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(
        body["data"]["ledger"]["a1BTotal"], "a1_b_total:4:1",
        "{body}"
    );
    assert_eq!(
        body["data"]["ledger"]["a1bTotal"], "a_1b_total:4:2",
        "{body}"
    );
}
