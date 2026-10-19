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

/// A schema with no guard at its request — no global pool, no endpoint guards —
/// is **unguarded**, and the boot says so once — a warn, since a public read
/// API is a legitimate shape.
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
            "no guard runs at the /graphql request — graphql operations run unguarded",
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

    resp.assert_status(nest_rs_http::StatusCode::BAD_REQUEST);
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

    resp.assert_status(nest_rs_http::StatusCode::BAD_REQUEST);
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

    resp.assert_status(nest_rs_http::StatusCode::SERVICE_UNAVAILABLE);
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

/// The guards at the `/graphql` request read nestrs's request and attach what
/// they read, a seed forwards it into the operation's context, and a refusal is
/// the edge's problem document, its reason the `detail`.
mod the_endpoint_guards_speak_the_request_vocabulary {
    use nest_rs_core::{AmbientValue, Container, Layer, UnitContext, injectable, module};
    use nest_rs_graphql::async_graphql::{Context, Request as GqlRequest, Result as GqlResult};
    use nest_rs_graphql::{
        GraphqlContextSeed, GraphqlModule, GraphqlView, SeedLifetime, operations, resolver,
    };
    use nest_rs_guards::{Denial, Guard, GuardFor};
    use nest_rs_http::{EndpointGuards, HttpView, Request, async_trait};
    use nest_rs_testing::TestApp;

    #[derive(Clone)]
    struct Tenant(String);

    impl AmbientValue for Tenant {}

    /// Admits a request naming its tenant, attaching it; refuses one that names
    /// none.
    #[injectable]
    #[derive(Default)]
    struct TenantGate;

    impl Layer for TenantGate {}

    #[async_trait]
    impl Guard for TenantGate {
        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            let Some(tenant) = HttpView::of(cx)?
                .request()
                .header("x-tenant")
                .map(str::to_owned)
            else {
                return Err(Denial::forbidden("name a tenant"));
            };
            cx.attach(Tenant(tenant));
            Ok(())
        }
    }

    impl GuardFor<HttpView> for TenantGate {}

    /// Forwards what the gate attached.
    fn forward_tenant(_req: &Request, _container: &Container, gql: GqlRequest) -> GqlRequest {
        match nest_rs_core::ambient::<Tenant>() {
            Some(tenant) => gql.data((*tenant).clone()),
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
        providers = [TenantResolver, TenantGate, EndpointGuards<GraphqlView, (TenantGate,)>],
    )]
    struct TenantModule;

    #[tokio::test]
    async fn a_seed_forwards_what_the_endpoint_guards_attached() {
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
    async fn an_endpoint_guards_refusal_is_the_edges_problem_document() {
        let app = TestApp::for_module::<TenantModule>().await.expect("boots");

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ tenant }" }))
            .send()
            .await;

        resp.assert_status(nest_rs_http::StatusCode::FORBIDDEN);
        resp.json()
            .await
            .value()
            .object()
            .get("detail")
            .assert_string("name a tenant");
    }
}

/// What the POST answers on the wire, as 6.x answered it through
/// async-graphql-poem 7.2.1: JSON, `200` whatever `errors` holds,
/// `cache-control` from the schema's hints only when every operation
/// succeeded, then every header a resolver set.
mod the_answer_on_the_wire {
    use nest_rs_core::module;
    use nest_rs_graphql::async_graphql::{self, Context, Upload};
    use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
    use nest_rs_http::{HttpConfig, HttpModule, StatusCode, header};
    use nest_rs_testing::{TestApp, TestForm, TestFormField, TestResponse};
    use serde_json::json;

    #[derive(async_graphql::SimpleObject)]
    #[graphql(cache_control(max_age = 60))]
    struct Cached {
        value: i32,
    }

    #[resolver]
    struct WireResolver;

    #[operations]
    impl WireResolver {
        #[query]
        #[public]
        async fn cached(&self) -> Cached {
            Cached { value: 1 }
        }

        #[query]
        #[public]
        async fn tagged(&self, ctx: &Context<'_>) -> i32 {
            ctx.append_http_header("x-graphql-tag", "first");
            ctx.append_http_header("x-graphql-tag", "second");
            2
        }

        #[query]
        #[public]
        async fn refused(&self) -> async_graphql::Result<i32> {
            Err(async_graphql::Error::new("refused on purpose"))
        }

        #[mutation]
        #[public]
        async fn upload_size(
            &self,
            ctx: &Context<'_>,
            file: Upload,
        ) -> async_graphql::Result<usize> {
            let upload = file
                .value(ctx)
                .map_err(|unread| async_graphql::Error::new(unread.to_string()))?;
            Ok(upload.content.len())
        }
    }

    /// The cap the multipart tests send under and over.
    const CAP: usize = 4096;

    #[module(
        imports = [
            HttpModule::for_root(HttpConfig { max_body_bytes: Some(CAP), ..HttpConfig::default() }),
            GraphqlModule::for_root(GraphqlConfig { max_batch_size: 2, ..GraphqlConfig::default() }),
        ],
        providers = [WireResolver],
    )]
    struct WireModule;

    async fn boot() -> TestApp {
        TestApp::for_module::<WireModule>().await.expect("boots")
    }

    fn values(resp: &TestResponse, name: header::HeaderName) -> Vec<String> {
        resp.0
            .headers()
            .get_all(name)
            .iter()
            .map(|value| value.to_str().expect("a visible header").to_owned())
            .collect()
    }

    async fn body(resp: TestResponse) -> serde_json::Value {
        let text = resp.0.into_body().into_string().await.expect("a body");
        serde_json::from_str(&text).unwrap_or_else(|_| panic!("a JSON body: {text}"))
    }

    #[tokio::test]
    async fn the_response_header_set_matches_6x() {
        let app = boot().await;

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&json!({ "query": "{ cached { value } tagged }" }))
            .send()
            .await;

        resp.assert_status_is_ok();
        let mut names: Vec<&str> = resp.0.headers().keys().map(|name| name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, SIX_X_HEADER_SET);
        assert_eq!(
            values(&resp, header::CONTENT_TYPE),
            ["application/json; charset=utf-8"]
        );
        assert_eq!(values(&resp, header::CACHE_CONTROL), ["max-age=60"]);
        assert_eq!(
            values(&resp, header::HeaderName::from_static("x-graphql-tag")),
            ["first", "second"],
            "every value a resolver appended, in order",
        );
        assert_eq!(
            body(resp).await,
            json!({ "data": { "cached": { "value": 1 }, "tagged": 2 } })
        );
    }

    /// The header names a POST answering `cached` and `tagged` carried in 6.x.
    const SIX_X_HEADER_SET: [&str; 9] = [
        "cache-control",
        "content-type",
        "cross-origin-opener-policy",
        "cross-origin-resource-policy",
        "referrer-policy",
        "traceresponse",
        "x-content-type-options",
        "x-frame-options",
        "x-graphql-tag",
    ];

    #[tokio::test]
    async fn an_answer_holding_errors_is_200_and_carries_no_cache_control() {
        let app = boot().await;

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&json!({ "query": "{ cached { value } refused }" }))
            .send()
            .await;

        resp.assert_status_is_ok();
        resp.assert_header_is_not_exist(header::CACHE_CONTROL);
        let body = body(resp).await;
        assert_eq!(body["errors"][0]["message"], "refused on purpose", "{body}");
    }

    #[tokio::test]
    async fn a_batch_answers_each_operation_in_order_with_every_header_set() {
        let app = boot().await;

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&json!([{ "query": "{ tagged }" }, { "query": "{ refused }" }]))
            .send()
            .await;

        resp.assert_status_is_ok();
        resp.assert_header_is_not_exist(header::CACHE_CONTROL);
        assert_eq!(
            values(&resp, header::HeaderName::from_static("x-graphql-tag")),
            ["first", "second"],
        );
        let body = body(resp).await;
        assert_eq!(body[0], json!({ "data": { "tagged": 2 } }), "{body}");
        assert_eq!(
            body[1]["errors"][0]["message"], "refused on purpose",
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_batch_past_its_size_is_413_and_executes_nothing() {
        let app = boot().await;
        let operation = json!({ "query": "{ tagged }" });

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&json!([operation, operation, operation]))
            .send()
            .await;

        resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
        resp.assert_content_type("application/problem+json");
        resp.assert_header_is_not_exist("x-graphql-tag");
    }

    /// The GraphQL multipart request specification's three parts: the
    /// operation, the map of files onto its variables, and the file.
    fn upload_of(bytes: usize) -> TestForm {
        TestForm::new()
            .text(
                "operations",
                json!({
                    "query": "mutation ($file: Upload!) { uploadSize(file: $file) }",
                    "variables": { "file": null },
                })
                .to_string(),
            )
            .text("map", json!({ "0": ["variables.file"] }).to_string())
            .field(
                TestFormField::bytes(vec![7_u8; bytes])
                    .name("0")
                    .filename("notes.txt"),
            )
    }

    #[tokio::test]
    async fn a_multipart_upload_under_the_cap_reaches_its_resolver() {
        let app = boot().await;

        let resp = app
            .http()
            .post("/graphql")
            .multipart(upload_of(1024))
            .send()
            .await;

        resp.assert_status_is_ok();
        assert_eq!(body(resp).await, json!({ "data": { "uploadSize": 1024 } }));
    }

    /// A refusal of the multipart reader keeps its fixed sentence.
    #[tokio::test]
    async fn a_multipart_request_without_its_map_is_refused_naming_the_part() {
        let app = boot().await;
        let form = TestForm::new().text("operations", json!({ "query": "{ tagged }" }).to_string());

        let resp = app.http().post("/graphql").multipart(form).send().await;

        resp.assert_status(StatusCode::BAD_REQUEST);
        resp.assert_content_type("application/json");
        assert_eq!(
            body(resp).await,
            json!({ "errors": [{ "message": "the body is not a GraphQL request: Missing \"map\" part" }] })
        );
    }

    #[tokio::test]
    async fn a_multipart_upload_over_the_cap_is_413() {
        let app = boot().await;

        app.http()
            .post("/graphql")
            .multipart(upload_of(CAP * 16))
            .send()
            .await
            .assert_status(StatusCode::PAYLOAD_TOO_LARGE);
    }
}
