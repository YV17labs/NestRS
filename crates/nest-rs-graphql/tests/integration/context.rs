//! Per-request context bridge: a value an HTTP guard attaches to the request
//! reaches a GraphQL resolver — end-to-end through the harness.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::Context;
use nest_rs_graphql::{GraphqlContextSeed, GraphqlModule, SeedLifetime, operations, resolver};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::async_trait;
use nest_rs_testing::TestApp;
use poem::Request;

#[derive(Clone)]
struct RequestTag(String);

#[injectable]
#[derive(Default)]
struct TagGuard;

impl Layer for TagGuard {}

#[async_trait]
impl Guard for TagGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        req.extensions_mut().insert(RequestTag("hello".into()));
        Ok(())
    }
}

impl HttpGuard for TagGuard {}

#[resolver]
struct TagResolver;

nest_rs_core::inventory::submit! {
    GraphqlContextSeed {
        lifetime: SeedLifetime::Connection,
        owner_type_id: || Some(std::any::TypeId::of::<TagResolver>()),
        seed: |req, _container, gql| match req.extensions().get::<RequestTag>() {
            Some(tag) => gql.data(tag.clone()),
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
async fn resolver_reads_a_per_request_value_bridged_from_the_poem_request() {
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

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ anyone }" }))
            .send()
            .await;
        resp.assert_status_is_ok();

        let event = logs
            .find(
                "nest_rs::graphql",
                "no operation guard registered — graphql operations run unguarded",
            )
            .into_iter()
            .next()
            .expect("the boot announces an unguarded schema");
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
    use nest_rs_testing::{LogCapture, TestApp};
    use poem::{Request, Response};

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
