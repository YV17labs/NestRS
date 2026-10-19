//! A request-scoped provider reached through a [`Scoped<T>`] parameter is one
//! instance per request, shared across its fields, and inside the access
//! contract the boot checks.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use nest_rs_core::{injectable, module};
use nest_rs_graphql::async_graphql::SimpleObject;
use nest_rs_graphql::{GraphqlModule, Scoped, operations, resolver};
use nest_rs_testing::TestApp;

/// A singleton source of monotonic stamps, shared across every request.
#[injectable]
#[derive(Default)]
struct Ticker {
    next: AtomicU64,
}

impl Ticker {
    fn stamp(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }
}

/// Request-scoped; `id()` stamps from the singleton [`Ticker`] on first read and
/// memoizes it.
#[injectable(scope = request)]
struct Probe {
    #[inject]
    ticker: Arc<Ticker>,
    stamp: OnceLock<u64>,
}

impl Probe {
    fn id(&self) -> u64 {
        *self.stamp.get_or_init(|| self.ticker.stamp())
    }
}

#[resolver]
struct ProbeResolver;

#[operations]
impl ProbeResolver {
    #[query]
    #[public]
    async fn first(&self, probe: Scoped<Probe>) -> String {
        probe.id().to_string()
    }

    #[query]
    #[public]
    async fn second(&self, probe: Scoped<Probe>) -> String {
        probe.id().to_string()
    }

    #[query]
    #[public]
    async fn stamped(&self) -> Stamped {
        Stamped { label: "one" }
    }

    #[field_resolver]
    async fn stamp(&self, parent: &Stamped, probe: Scoped<Probe>) -> String {
        format!("{} {}", parent.label, probe.id())
    }
}

#[derive(SimpleObject)]
#[graphql(complex)]
struct Stamped {
    label: &'static str,
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [Ticker, Probe, ProbeResolver],
)]
struct ScopeTestModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<ScopeTestModule>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

async fn query_field(app: &TestApp, field: &str) -> String {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": format!("{{ {field} }}") }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.json()
        .await
        .value()
        .object()
        .get("data")
        .object()
        .get(field)
        .string()
        .to_owned()
}

#[tokio::test]
async fn two_fields_of_one_operation_share_one_request_scoped_instance() {
    let app = boot().await;

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ first second }" }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let data = json.value().object().get("data").object();
    let first = data.get("first").string();
    let second = data.get("second").string();

    assert_eq!(
        first, second,
        "both fields resolve the same request-scoped instance within one operation",
    );
}

#[tokio::test]
async fn a_fresh_operation_builds_a_new_request_scoped_instance() {
    let app = boot().await;

    let a = query_field(&app, "first").await;
    let b = query_field(&app, "first").await;

    assert_ne!(
        a, b,
        "a request-scoped instance must not carry across operations",
    );
}

#[tokio::test]
async fn a_field_resolver_reaches_the_request_scoped_instance_its_root_field_did() {
    let app = boot().await;

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ first stamped { stamp } }" }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json: serde_json::Value = resp.json().await.value().deserialize();
    let first = json["data"]["first"].as_str().expect("a stamp");
    assert_eq!(
        json["data"]["stamped"]["stamp"],
        format!("one {first}"),
        "{json}",
    );
}

/// A `Scoped<T>` parameter's provider is checked at boot like an `#[inject]`ed
/// one: unimported, or offered by no module, it refuses the boot.
mod access_contract {
    use nest_rs_core::{AccessGraphError, MissingDependencyError, injectable, module};
    use nest_rs_graphql::async_graphql::SimpleObject;
    use nest_rs_graphql::{GraphqlModule, Scoped, operations, resolver};
    use nest_rs_testing::TestApp;

    #[injectable(scope = request)]
    #[derive(Default)]
    struct AuditTrail;

    impl AuditTrail {
        fn entries(&self) -> i32 {
            0
        }
    }

    #[module(providers = [AuditTrail])]
    struct AuditTrailModule;

    #[resolver]
    struct StrayAuditResolver;

    #[operations]
    impl StrayAuditResolver {
        #[query]
        #[public]
        async fn audit_entries(&self, trail: Scoped<AuditTrail>) -> i32 {
            trail.entries()
        }
    }

    #[module(imports = [GraphqlModule::for_root(None)], providers = [StrayAuditResolver])]
    struct StrayAuditModule;

    #[module(imports = [AuditTrailModule, StrayAuditModule])]
    struct StrayAuditRoot;

    #[tokio::test]
    async fn a_resolver_reaching_a_provider_its_module_does_not_import_fails_the_boot() {
        let refused = TestApp::for_module::<StrayAuditRoot>()
            .await
            .err()
            .expect("the field would reach a provider its module does not import");
        let leak = refused
            .downcast::<AccessGraphError>()
            .unwrap_or_else(|refused| panic!("not the access violation: {refused:#}"));
        assert_eq!(leak.consumer, "StrayAuditResolver");
        assert_eq!(leak.owner, "AuditTrailModule");
    }

    /// Request-scoped, and in no module.
    #[injectable(scope = request)]
    #[derive(Default)]
    struct Unprovided;

    #[resolver]
    struct UnprovidedResolver;

    #[operations]
    impl UnprovidedResolver {
        #[query]
        #[public]
        async fn unprovided(&self, value: Scoped<Unprovided>) -> bool {
            let _ = value;
            true
        }
    }

    #[module(imports = [GraphqlModule::for_root(None)], providers = [UnprovidedResolver])]
    struct UnprovidedModule;

    #[tokio::test]
    async fn a_resolver_reaching_a_provider_no_module_offers_fails_the_boot_naming_it() {
        let refused = TestApp::for_module::<UnprovidedModule>()
            .await
            .err()
            .expect("the field would reach a provider no module offers");
        let missing = refused
            .downcast::<MissingDependencyError>()
            .unwrap_or_else(|refused| panic!("not the unmet dependency: {refused:#}"));
        assert_eq!(missing.consumer, "UnprovidedResolver");
        assert!(
            missing.dependency.ends_with("::Unprovided"),
            "{}",
            missing.dependency,
        );
    }

    #[resolver]
    struct DestructuringResolver;

    #[operations]
    impl DestructuringResolver {
        #[query]
        #[public]
        async fn trail_length(&self, Scoped(trail): Scoped<AuditTrail>) -> i32 {
            trail.entries()
        }
    }

    #[module(imports = [GraphqlModule::for_root(None)], providers = [DestructuringResolver])]
    struct DestructuringModule;

    #[module(imports = [AuditTrailModule, DestructuringModule])]
    struct DestructuringRoot;

    /// A pattern binds the parameter, never hides it: `Scoped(x): Scoped<T>`
    /// is checked as `x: Scoped<T>` is.
    #[tokio::test]
    async fn a_destructured_scoped_parameter_is_inside_the_access_contract() {
        let refused = TestApp::for_module::<DestructuringRoot>()
            .await
            .err()
            .expect("the field would reach a provider its module does not import");
        let leak = refused
            .downcast::<AccessGraphError>()
            .unwrap_or_else(|refused| panic!("not the access violation: {refused:#}"));
        assert_eq!(leak.consumer, "DestructuringResolver");
        assert_eq!(leak.owner, "AuditTrailModule");
    }

    #[derive(SimpleObject)]
    #[graphql(complex)]
    struct Ledger {
        balance: i32,
    }

    #[resolver]
    struct LedgerResolver;

    #[operations]
    impl LedgerResolver {
        #[query]
        #[public]
        async fn ledger(&self) -> Ledger {
            Ledger { balance: 1 }
        }

        #[field_resolver]
        async fn audited(&self, parent: &Ledger, Scoped(trail): Scoped<AuditTrail>) -> i32 {
            parent.balance + trail.entries()
        }
    }

    #[module(imports = [GraphqlModule::for_root(None)], providers = [LedgerResolver])]
    struct LedgerModule;

    #[module(imports = [AuditTrailModule, LedgerModule])]
    struct LedgerRoot;

    #[tokio::test]
    async fn a_field_resolvers_destructured_scoped_parameter_is_inside_the_access_contract() {
        let refused = TestApp::for_module::<LedgerRoot>()
            .await
            .err()
            .expect("the field resolver would reach a provider its module does not import");
        let leak = refused
            .downcast::<AccessGraphError>()
            .unwrap_or_else(|refused| panic!("not the access violation: {refused:#}"));
        assert_eq!(leak.consumer, "LedgerResolver");
        assert_eq!(leak.owner, "AuditTrailModule");
    }
}
