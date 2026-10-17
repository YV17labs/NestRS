//! Per-request context bridge: a value an HTTP guard attaches to the
//! `/graphql` request reaches a GraphQL resolver — end-to-end through the
//! harness.

use nest_rs_core::{AmbientValue, Layer, injectable, module};
use nest_rs_graphql::async_graphql::Context;
use nest_rs_graphql::{GraphqlContextSeed, GraphqlModule, SeedLifetime, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::async_trait;
use nest_rs_testing::TestApp;

#[derive(Clone)]
struct RequestTag(String);

impl AmbientValue for RequestTag {}

#[injectable]
#[derive(Default)]
struct TagGuard;

impl Layer for TagGuard {}

#[async_trait]
impl Guard for TagGuard {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(RequestTag("hello".into()));
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for TagGuard {}

#[resolver]
struct TagResolver;

nest_rs_core::inventory::submit! {
    GraphqlContextSeed {
        lifetime: SeedLifetime::Connection,
        owner_type_id: || Some(std::any::TypeId::of::<TagResolver>()),
        seed: |_req, _container, gql| match nest_rs_core::ambient::<RequestTag>() {
            Some(tag) => gql.data(RequestTag::clone(&tag)),
            None => gql,
        },
    }
}

#[operations]
impl TagResolver {
    #[query]
    #[public]
    async fn tag(&self, ctx: &Context<'_>) -> String {
        ctx.data_opt::<RequestTag>()
            .map(|t| t.0.clone())
            .unwrap_or_else(|| "none".into())
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [TagGuard, TagResolver])]
struct GraphqlTestModule;

#[tokio::test]
async fn resolver_reads_a_per_request_value_a_guard_attached() {
    let app = TestApp::builder()
        .module::<GraphqlTestModule>()
        .use_guards_global([guard::<TagGuard>()])
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ tag }" }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let tag = json
        .value()
        .object()
        .get("data")
        .object()
        .get("tag")
        .string();
    assert_eq!(tag, "hello");
}

/// A schema with no operation guard and no global pool is **unguarded**, and the
/// boot says so once — a warn, since a public read API is a legitimate shape.
mod an_unguarded_schema_announces_itself {
    use nest_rs_core::module;
    use nest_rs_graphql::{GraphqlModule, operations, resolver};
    use nest_rs_testing::{LogCapture, TestApp};

    #[resolver]
    struct OpenResolver;

    #[operations]
    impl OpenResolver {
        #[query]
        #[public]
        async fn anyone(&self) -> nest_rs_graphql::async_graphql::Result<String> {
            Ok("open".into())
        }
    }

    #[module(imports = [GraphqlModule::for_root(None)], providers = [OpenResolver])]
    struct OpenModule;

    #[tokio::test]
    async fn at_warn_naming_the_mode() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<OpenModule>()
            .await
            .expect("an unguarded schema boots — that is the point");

        assert_unguarded(&app, &logs).await;
    }

    /// An empty pool gates nothing, so it is announced as no pool at all.
    #[tokio::test]
    async fn with_an_empty_global_pool_too() {
        let logs = LogCapture::install();
        let app = TestApp::builder()
            .module::<OpenModule>()
            .use_guards_global([])
            .build()
            .await
            .expect("an unguarded schema boots under an empty pool");

        assert_unguarded(&app, &logs).await;
        logs.expect_none("nest_rs::graphql", "graphql operations gated");
    }

    async fn assert_unguarded(app: &TestApp, logs: &LogCapture) {
        let resp = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ anyone }" }))
            .send()
            .await;
        resp.assert_status_is_ok();

        let event = logs.expect_one(
            "nest_rs::graphql",
            "no operation guard registered — graphql operations run unguarded",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("mode").as_deref(), Some("unguarded"));
    }
}

/// A `#[resolver]` no module lists is filtered from the schema, and the boot
/// warns with the remedy.
mod a_resolver_no_module_lists {
    use nest_rs_core::module;
    use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
    use nest_rs_testing::{LogCapture, TestApp};

    #[resolver]
    struct OrphanResolver;

    #[operations]
    impl OrphanResolver {
        #[query]
        #[public]
        async fn orphaned(&self) -> nest_rs_graphql::async_graphql::Result<String> {
            Ok("never reachable".into())
        }
    }

    #[resolver]
    struct ListedResolver;

    #[operations]
    impl ListedResolver {
        #[query]
        #[public]
        async fn listed(&self) -> nest_rs_graphql::async_graphql::Result<String> {
            Ok("reachable".into())
        }
    }

    #[module(imports = [GraphqlModule::for_root(None)], providers = [ListedResolver])]
    struct PartialModule;

    #[module(
        imports = [GraphqlModule::for_root(GraphqlConfig {
            strict_resolver_membership: true,
            ..GraphqlConfig::default()
        })],
        providers = [ListedResolver],
    )]
    struct StrictModule;

    #[tokio::test]
    async fn is_reported_at_warn_with_the_line_that_would_fix_it() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<PartialModule>()
            .await
            .expect("an app with an unlisted resolver still boots");

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ orphaned }" }))
            .send()
            .await;
        let body = resp.0.into_body().into_string().await.unwrap_or_default();
        assert!(
            body.contains("orphaned"),
            "the query is rejected by name: {body}",
        );

        // Link-time: every resolver in this test binary is a candidate.
        let reported = logs.find(
            nest_rs_graphql::TARGET,
            "unreachable resolver skipped from the GraphQL schema",
        );
        let ours = reported
            .iter()
            .find(|event| event.field("resolver").as_deref() == Some("OrphanResolver"))
            .unwrap_or_else(|| panic!("the unlisted resolver is named: {reported:#?}"));
        assert_eq!(ours.level, "warn");
        assert!(
            ours.field("hint").is_some_and(|h| h.contains("providers")),
            "and the remedy rides along, got {:?}",
            ours.fields,
        );
        assert!(
            !reported
                .iter()
                .any(|event| event.field("resolver").as_deref() == Some("ListedResolver")),
            "a listed resolver is never reported: {reported:#?}",
        );
    }

    #[tokio::test]
    async fn is_a_boot_failure_when_the_app_asked_for_one() {
        let err = TestApp::for_module::<StrictModule>()
            .await
            .err()
            .expect("strict membership refuses the boot");
        let message = format!("{err:#}");
        assert!(
            message.contains("OrphanResolver"),
            "the boot names the resolver it refused over: {message}",
        );
        assert!(
            message.contains("providers"),
            "and carries the same remedy the warn does: {message}",
        );
    }
}

// An app bridge whose `around` never drives the operation must not yield an
// empty `200`.

mod an_operation_guard_that_never_runs_the_operation {
    use nest_rs_core::{injectable, module};
    use nest_rs_graphql::async_graphql::Result as GqlResult;
    use nest_rs_graphql::{BoxFuture, GraphqlModule, GraphqlOperationGuard, operations, resolver};
    use nest_rs_http::{Request, Response};
    use nest_rs_testing::{LogCapture, TestApp};

    /// Returns without ever polling `inner`, so the operation never executes.
    #[injectable]
    #[derive(Default)]
    struct NeverRunsTheOperation;

    impl GraphqlOperationGuard for NeverRunsTheOperation {
        fn before<'a>(&'a self, _req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>> {
            Box::pin(async move { Ok(()) })
        }

        fn around<'a>(&'a self, _req: &'a Request, _inner: BoxFuture<'a, ()>) -> BoxFuture<'a, ()> {
            Box::pin(async {})
        }
    }

    #[resolver]
    struct EchoResolver;

    #[operations]
    impl EchoResolver {
        #[query]
        #[public]
        async fn echo(&self) -> GqlResult<String> {
            Ok("echoed".into())
        }
    }

    #[module(
        imports = [GraphqlModule::for_root(None)],
        providers = [EchoResolver, NeverRunsTheOperation as dyn GraphqlOperationGuard],
    )]
    struct BrokenBridgeModule;

    #[tokio::test]
    async fn is_reported_rather_than_answered_with_nothing() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<BrokenBridgeModule>()
            .await
            .expect("a bridge that misbehaves at runtime still boots");

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ echo }" }))
            .send()
            .await;
        resp.assert_status(poem::http::StatusCode::INTERNAL_SERVER_ERROR);
        let body = resp
            .0
            .into_body()
            .into_string()
            .await
            .expect("a response body");
        assert!(
            !body.contains("echoed"),
            "the operation really did not run: {body}",
        );

        let event = logs.expect_one(
            "nest_rs::graphql",
            "the guarded operation produced no response",
        );
        assert_eq!(event.level, "error");
        assert_eq!(
            event.field("reason").as_deref(),
            Some("no_response"),
            "{:?}",
            event.fields,
        );
    }
}

/// A body that is JSON but not a GraphQL request — `variables` as a string.
#[tokio::test]
async fn a_body_that_is_not_a_graphql_request_is_answered_with_an_error_entry_and_no_value() {
    let app = TestApp::builder()
        .module::<GraphqlTestModule>()
        .build()
        .await
        .expect("boots");

    let resp = app
        .http()
        .post("/graphql")
        .header("content-type", "application/json")
        .body(r#"{"query":"{ tag }","variables":"sk_live_secret"}"#)
        .send()
        .await;

    resp.assert_status(poem::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(
        &resp
            .0
            .into_body()
            .into_string()
            .await
            .expect("a response body"),
    )
    .expect("a GraphQL response is JSON");
    let message = body["errors"][0]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an `errors` entry with a message: {body}"));
    assert!(
        message.contains("invalid type: a string, expected"),
        "the decode failure is said: {message}",
    );
    assert!(!message.contains("sk_live"), "without the value: {message}");
    assert!(body.get("data").is_none(), "nothing executed: {body}");
}

/// A multipart request whose `map` part is not a map of files: serde's sentence
/// would quote the part.
#[tokio::test]
async fn a_multipart_part_that_does_not_decode_is_answered_without_its_value() {
    let app = TestApp::builder()
        .module::<GraphqlTestModule>()
        .build()
        .await
        .expect("boots");
    let body = "--nestrs\r\n\
        content-disposition: form-data; name=\"operations\"\r\n\r\n\
        {\"query\":\"{ tag }\"}\r\n\
        --nestrs\r\n\
        content-disposition: form-data; name=\"map\"\r\n\r\n\
        {\"0\":\"sk_live_secret\"}\r\n\
        --nestrs--\r\n";

    let resp = app
        .http()
        .post("/graphql")
        .header("content-type", "multipart/form-data; boundary=nestrs")
        .body(body)
        .send()
        .await;

    resp.assert_status(poem::http::StatusCode::BAD_REQUEST);
    let body = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a response body");
    assert!(!body.contains("sk_live"), "{body}");
    assert!(
        body.contains("the `map` part: invalid type: a string"),
        "{body}"
    );
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [crate::RequestStoreDown, TagResolver],
)]
struct RequestStoreDownModule;

/// A pooled guard that cannot evaluate the `/graphql` request files what failed
/// on the request's denial line, and the client reads the condition alone.
#[tokio::test]
async fn a_request_denial_keeping_its_cause_files_the_chain() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::builder()
        .module::<RequestStoreDownModule>()
        .use_guards_global([guard::<crate::RequestStoreDown>()])
        .build()
        .await
        .expect("boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ tag }" }))
        .send()
        .await;

    resp.assert_status(poem::http::StatusCode::SERVICE_UNAVAILABLE);
    let body = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a response body");
    assert!(
        !body.contains("refused"),
        "the cause stays on the server: {body}"
    );
    let event = logs.expect_one("nest_rs::layers", "guard denied the request");
    assert_eq!(event.field("status").as_deref(), Some("503"));
    assert_eq!(
        event.field("error").as_deref(),
        Some(crate::STORE_DOWN_CHAIN)
    );
}

/// The in-band seams read and answer on nestrs's request: an operation guard
/// reads the head and keeps what it reads on the request, a seed forwards it,
/// and a refusal is the guard's own response, its text said as a problem
/// document's `detail` as the edge says every plain-text error.
mod the_in_band_seams_speak_the_request_vocabulary {
    use nest_rs_core::{Container, injectable, module};
    use nest_rs_graphql::async_graphql::{Context, Request as GqlRequest, Result as GqlResult};
    use nest_rs_graphql::{
        BoxFuture, GraphqlContextSeed, GraphqlModule, GraphqlOperationGuard, SeedLifetime,
        operations, resolver,
    };
    use nest_rs_http::{HeaderValue, Request, Response, StatusCode, header};
    use nest_rs_testing::TestApp;

    #[derive(Clone)]
    struct Tenant(String);

    /// Admits a request naming its tenant, kept on the request; refuses one
    /// that names none with an answer of its own.
    #[injectable]
    #[derive(Default)]
    struct TenantGate;

    impl GraphqlOperationGuard for TenantGate {
        fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>> {
            Box::pin(async move {
                let Some(tenant) = req.header("x-tenant").map(str::to_owned) else {
                    return Err(Response::builder()
                        .status(StatusCode::FORBIDDEN)
                        .header(header::WWW_AUTHENTICATE, HeaderValue::from_static("Tenant"))
                        .body("name a tenant"));
                };
                req.extensions_mut().insert(Tenant(tenant));
                Ok(())
            })
        }

        fn around<'a>(&'a self, req: &'a Request, inner: BoxFuture<'a, ()>) -> BoxFuture<'a, ()> {
            assert!(
                req.extensions().get::<Tenant>().is_some(),
                "`around` reads the request `before` admitted",
            );
            inner
        }
    }

    /// Forwards what the gate kept on the request.
    fn forward_tenant(req: &Request, _container: &Container, gql: GqlRequest) -> GqlRequest {
        match req.extensions().get::<Tenant>() {
            Some(tenant) => gql.data(tenant.clone()),
            None => gql,
        }
    }

    nest_rs_core::inventory::submit! {
        GraphqlContextSeed {
            lifetime: SeedLifetime::Request,
            owner_type_id: || Some(std::any::TypeId::of::<TenantResolver>()),
            seed: forward_tenant,
        }
    }

    #[resolver]
    struct TenantResolver;

    #[operations]
    impl TenantResolver {
        #[query]
        #[public]
        async fn tenant(&self, ctx: &Context<'_>) -> GqlResult<String> {
            Ok(ctx.data::<Tenant>()?.0.clone())
        }
    }

    #[module(
        imports = [GraphqlModule::for_root(None)],
        providers = [TenantResolver, TenantGate as dyn GraphqlOperationGuard],
    )]
    struct TenantModule;

    #[tokio::test]
    async fn a_seed_forwards_what_the_operation_guard_read_off_the_request() {
        let app = TestApp::for_module::<TenantModule>().await.expect("boots");

        let resp = app
            .http()
            .post("/graphql")
            .header("x-tenant", "acme")
            .body_json(&serde_json::json!({ "query": "{ tenant }" }))
            .send()
            .await;

        resp.assert_json(serde_json::json!({ "data": { "tenant": "acme" } }))
            .await;
    }

    #[tokio::test]
    async fn an_operation_guards_refusal_is_its_own_response() {
        let app = TestApp::for_module::<TenantModule>().await.expect("boots");

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ tenant }" }))
            .send()
            .await;

        resp.assert_status(poem::http::StatusCode::FORBIDDEN);
        resp.assert_header("www-authenticate", "Tenant");
        resp.json()
            .await
            .value()
            .object()
            .get("detail")
            .assert_string("name a tenant");
    }
}
