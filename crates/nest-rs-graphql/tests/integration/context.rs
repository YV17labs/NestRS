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

nest_rs_graphql::inventory::submit! {
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

/// A schema with no operation guard and no global pool is an **unguarded**
/// schema, and the boot says so once.
///
/// It is a warn rather than a refusal because an app with no authn posture at
/// all is a legitimate shape — a public read API, a demo. What makes the line
/// load-bearing is the shape next door: an app that *meant* to import its authz
/// bridge and did not gets a schema that answers every operation, with no
/// status code, no error frame and no failing test to notice it. This is the
/// GraphQL twin of the MCP deny-all announcement, and the two differ on purpose
/// — MCP fails closed, GraphQL falls open, so only one of them can afford to be
/// quiet, and it is not this one.
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

        // It really is open: the operation answers with no credential at all.
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

/// A `#[resolver]` no module lists is **silently filtered** from the schema.
///
/// Module-gating is what makes per-app subsets work, so this is correct
/// behaviour — and indistinguishable from a resolver whose queries were never
/// written. The app boots, the schema is smaller than the author thinks, and
/// the first sign is a client's `Unknown field` days later. The warn carries
/// the remedy because the fix is one line in a `#[module(providers = [...])]`.
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

    // `OrphanResolver` is deliberately absent from `providers`.
    #[module(imports = [GraphqlModule::for_root(None)], providers = [ListedResolver])]
    struct PartialModule;

    // The same hole, under an app that asked for it to be fatal.
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

        // The schema really is missing it — that is what the warn is about.
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

        // `inventory` is link-time, so every resolver in this test binary is a
        // candidate; what matters is that ours is named.
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

    /// `strict_resolver_membership` promotes that warn to a boot failure, for an
    /// app where a forgotten `providers` entry must not reach a deployment.
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

// --- an operation guard that never runs the operation -------------------------
//
// `GraphqlOperationGuard::around` is handed the operation as a future and owes
// it exactly one thing: drive it. An app bridge that returns early — an
// `if denied { return }` written without awaiting the inner future — leaves the
// endpoint with nothing to answer with.
//
// The failure that matters is the *quiet* one: without this line the endpoint
// serves an empty `200`, which a GraphQL client reads as a response carrying
// neither `data` nor `errors`. Every operation on that deployment silently
// answers nothing, and the app's own logs say a request came in and succeeded.

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
            // `inner` is dropped rather than awaited.
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
        // The status is the whole failure this branch exists to prevent: the
        // comment above calls it "an empty 200", and a body assertion alone
        // cannot tell an empty 200 from an empty 500. Asserted first, because
        // it is the half a reader would otherwise have to take on trust.
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
