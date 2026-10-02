//! `#[use_guards]` on a `#[resolver]` impl — end-to-end through the in-process
//! harness.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::{Context, Result, Result as GqlResult};
use nest_rs_graphql::{
    GraphqlContextSeed, GraphqlModule, GraphqlOperationContext, SeedLifetime, async_trait,
    operations, resolver,
};
use nest_rs_guards::{Denial, GraphqlGuard, Guard, HttpGuard, guard};
use nest_rs_http::async_trait as http_async_trait;
use nest_rs_testing::TestApp;
use poem::Request;
use poem::http::StatusCode;

#[derive(Clone)]
struct Role(String);

#[injectable]
#[derive(Default)]
struct RoleHeaderGuard;

impl Layer for RoleHeaderGuard {}

#[http_async_trait]
impl Guard for RoleHeaderGuard {
    async fn check_http(&self, req: &mut Request) -> std::result::Result<(), Denial> {
        if let Some(role) = req
            .headers()
            .get("x-role")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned())
        {
            req.extensions_mut().insert(Role(role));
        }
        Ok(())
    }
}

impl HttpGuard for RoleHeaderGuard {}

#[injectable]
#[derive(Default)]
struct RequireAdmin;

nest_rs_graphql::inventory::submit! {
    GraphqlContextSeed {
        lifetime: SeedLifetime::Connection,
        owner_type_id: || Some(std::any::TypeId::of::<RequireAdmin>()),
        seed: |req, _container, gql| match req.extensions().get::<Role>() {
            Some(role) => gql.data(role.clone()),
            None => gql,
        },
    }
}

impl Layer for RequireAdmin {}

#[async_trait]
impl Guard for RequireAdmin {
    async fn check_graphql(
        &self,
        op: &GraphqlOperationContext<'_>,
    ) -> std::result::Result<(), Denial> {
        match op.data_opt::<Role>() {
            Some(role) if role.0 == "admin" => Ok(()),
            _ => Err(Denial::forbidden("forbidden")),
        }
    }
}

impl GraphqlGuard for RequireAdmin {}

#[resolver]
#[use_guards(RequireAdmin)]
struct GuardedResolver;

// `secret` has no `&Context` of its own — the macro injects one to run the
// guard. `whoami` already declares one; the macro reuses it (the path the
// `#[crud]`-generated ops follow).
#[operations]
impl GuardedResolver {
    #[query]
    #[public]
    async fn secret(&self) -> Result<String> {
        Ok("classified".into())
    }

    #[query]
    #[public]
    async fn whoami(&self, ctx: &Context<'_>) -> Result<String> {
        Ok(ctx
            .data_opt::<Role>()
            .map(|r| r.0.clone())
            .unwrap_or_default())
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [RequireAdmin, RoleHeaderGuard, GuardedResolver])]
struct GuardedModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<GuardedModule>()
        .use_guards_global([guard::<RoleHeaderGuard>()])
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

#[tokio::test]
async fn resolver_guard_allows_an_admin() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .header("x-role", "admin")
        .body_json(&serde_json::json!({ "query": "{ secret }" }))
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    let json = resp.json().await;
    assert_eq!(
        json.value()
            .object()
            .get("data")
            .object()
            .get("secret")
            .string(),
        "classified",
    );

    // Reuse path: the guard runs on an op declaring its own `&Context`, and
    // the body still sees the seeded role.
    let who = app
        .http()
        .post("/graphql")
        .header("x-role", "admin")
        .body_json(&serde_json::json!({ "query": "{ whoami }" }))
        .send()
        .await;
    let who_json = who.json().await;
    assert_eq!(
        who_json
            .value()
            .object()
            .get("data")
            .object()
            .get("whoami")
            .string(),
        "admin",
    );
}

#[tokio::test]
async fn resolver_guard_denies_a_non_admin() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = boot().await;

    let resp = app
        .http()
        .post("/graphql")
        .header("x-role", "user")
        .body_json(&serde_json::json!({ "query": "{ secret }" }))
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    let json = resp.json().await;
    assert!(
        json.value().object().get_opt("errors").is_some(),
        "a non-admin is forbidden by the resolver guard",
    );

    // GraphQL answers a denial with `200 OK` and an error frame, so the HTTP
    // status carries nothing an operator can alert on. This event is the
    // structural floor `deny_http` has on the other side: every denial visible
    // at `warn`+ whatever the individual guard chose to log.
    let event = logs.expect_one("nest_rs::layers", "guard denied the operation");
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("guard")
            .is_some_and(|g| g.contains("RequireAdmin")),
        "the event names the guard that refused, got {:?}",
        event.fields,
    );
    assert_eq!(event.field("status").as_deref(), Some("403"));

    let anon = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ secret }" }))
        .send()
        .await;
    let anon_json = anon.json().await;
    assert!(
        anon_json.value().object().get_opt("errors").is_some(),
        "an anonymous request is forbidden by the resolver guard",
    );
}

// ---------------------------------------------------------------------------
// The chain runs whatever an operation returns. It was once emitted only for a
// `Result`-returning operation, so under a deny-all resolver guard `-> i32` and
// `-> Vec<String>` answered their data, and an app-wide guard protected only
// the operations that happened to be fallible. Every role is here — a query
// (async and sync), a subscription and a field resolver — because the gate was
// one condition shared by all of them.

/// Refuses every GraphQL operation it is asked about.
#[injectable]
#[derive(Default)]
struct DenyAll;

impl Layer for DenyAll {}

#[async_trait]
impl Guard for DenyAll {
    async fn check_graphql(
        &self,
        _op: &GraphqlOperationContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("denied by the resolver-scope guard"))
    }
}

impl GraphqlGuard for DenyAll {}
impl HttpGuard for DenyAll {}

/// Counts the GraphQL operations it is asked about, and lets each through.
#[injectable]
#[derive(Default)]
struct CountingGuard {
    seen: std::sync::atomic::AtomicUsize,
}

impl Layer for CountingGuard {}

#[async_trait]
impl Guard for CountingGuard {
    async fn check_graphql(
        &self,
        _op: &GraphqlOperationContext<'_>,
    ) -> std::result::Result<(), Denial> {
        self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

impl GraphqlGuard for CountingGuard {}
impl HttpGuard for CountingGuard {}

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Shelf {
    id: i32,
}

/// Extended only by [`SealedResolver`], and produced only by
/// [`ShelfResolver`]'s root — async-graphql takes one `#[ComplexObject]` per type.
#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Vault {
    id: i32,
}

/// What [`SealedResolver::sealed_search`] answers — named like a `Result`, and
/// an object.
#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
struct SealedSearchResult {
    hits: i32,
}

#[resolver]
struct ShelfResolver;

#[operations]
impl ShelfResolver {
    #[query]
    #[public]
    async fn shelves(&self) -> Vec<Shelf> {
        (1..=3).map(|id| Shelf { id }).collect()
    }

    #[query]
    #[public]
    async fn vaults(&self) -> Vec<Vault> {
        (1..=3).map(|id| Vault { id }).collect()
    }

    #[query]
    #[public]
    async fn open_count(&self) -> i32 {
        7
    }

    #[field_resolver]
    async fn label(&self, parent: &Shelf) -> String {
        format!("shelf-{}", parent.id)
    }
}

#[resolver]
#[use_guards(DenyAll)]
struct SealedResolver;

#[operations]
impl SealedResolver {
    #[query]
    #[public]
    async fn sealed_count(&self) -> i32 {
        42
    }

    #[query]
    #[public]
    async fn sealed_secrets(&self) -> Vec<String> {
        vec!["classified".into()]
    }

    #[query]
    #[public]
    fn sealed_sync(&self) -> i32 {
        42
    }

    #[query]
    #[public]
    async fn sealed_fallible(&self) -> Result<i32> {
        Ok(42)
    }

    /// A payload whose name ends in `Result`: a value, and guarded like one.
    #[query]
    #[public]
    async fn sealed_search(&self) -> SealedSearchResult {
        SealedSearchResult { hits: 42 }
    }

    /// A `Result` renamed on import: fallible, and guarded like one.
    #[query]
    #[public]
    async fn sealed_aliased(&self) -> GqlResult<i32> {
        Ok(42)
    }

    #[subscription]
    #[public]
    fn sealed_ticks(
        &self,
    ) -> impl nest_rs_graphql::async_graphql::futures_util::Stream<Item = i32> {
        nest_rs_graphql::async_graphql::futures_util::stream::iter([42])
    }

    /// Extends a type another resolver's root produces — a root that never ran
    /// this resolver's guards, so the field has to run them itself.
    #[field_resolver]
    async fn sealed_note(&self, parent: &Vault) -> String {
        format!("classified-{}", parent.id)
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [DenyAll, CountingGuard, ShelfResolver, SealedResolver],
)]
struct ChainModule;

async fn chain_app(global: Option<nest_rs_guards::GuardSpec>) -> TestApp {
    let builder = TestApp::builder()
        .module::<ChainModule>()
        .http(nest_rs_http::HttpTransport::new());
    match global {
        Some(spec) => builder.use_guards_global([spec]),
        None => builder,
    }
    .build()
    .await
    .expect("the schema boots and mounts at /graphql")
}

async fn query(app: &TestApp, document: &str) -> serde_json::Value {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": document }))
        .send()
        .await;
    resp.assert_status(StatusCode::OK);
    let text = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a GraphQL response body");
    serde_json::from_str(&text).expect("a GraphQL response is JSON")
}

fn assert_denied(body: &serde_json::Value, field: &str) {
    let rendered = body.to_string();
    assert!(
        !rendered.contains("classified") && !rendered.contains("42"),
        "`{field}` served its data past a deny-all guard: {rendered}",
    );
    let errors = body["errors"].as_array().cloned().unwrap_or_default();
    assert!(
        errors.iter().any(|e| e["extensions"]["code"] == "FORBIDDEN"
            && e["path"][e["path"]
                .as_array()
                .map_or(0, |p| p.len().saturating_sub(1))]
                == field),
        "`{field}` answers the guard's denial as a field error: {rendered}",
    );
}

#[tokio::test]
async fn a_resolver_guard_runs_on_every_operation_whatever_it_returns() {
    let app = chain_app(None).await;
    for field in [
        "sealedCount",
        "sealedSecrets",
        "sealedSync",
        "sealedFallible",
        "sealedAliased",
    ] {
        let body = query(&app, &format!("{{ {field} }}")).await;
        assert_denied(&body, field);
    }
    let body = query(&app, "{ sealedSearch { hits } }").await;
    assert_denied(&body, "sealedSearch");
    let body = query(&app, "{ vaults { id sealedNote } }").await;
    assert_denied(&body, "sealedNote");
}

#[tokio::test]
async fn a_resolver_guard_runs_on_a_bare_stream_subscription() {
    let app = chain_app(None).await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("sealed", "subscription { sealedTicks }");
    let message = socket
        .next_message()
        .await
        .expect("the subscription answers");
    let rendered = message.to_string();
    assert!(
        rendered.contains("denied by the resolver-scope guard") && !rendered.contains("42"),
        "the subscription is refused before it streams: {rendered}",
    );
}

#[tokio::test]
async fn an_app_wide_guard_runs_on_a_bare_return_operation() {
    let app = chain_app(Some(guard::<DenyAll>())).await;
    let body = query(&app, "{ openCount }").await;
    let rendered = body.to_string();
    assert!(
        !rendered.contains('7') && rendered.contains("FORBIDDEN"),
        "the app-wide guard refuses a bare-return operation: {rendered}",
    );
}

/// The pool runs once per root field, never once per parent a field resolver
/// extends: the root field that produced the parents already ran it in the same
/// request.
#[tokio::test]
async fn the_app_wide_pool_runs_once_per_root_field_not_per_parent() {
    let app = chain_app(Some(guard::<CountingGuard>())).await;
    let body = query(&app, "{ shelves { id label } }").await;
    assert_eq!(
        body["data"]["shelves"][2]["label"], "shelf-3",
        "the field resolver answers: {body}",
    );
    let counter = app
        .container()
        .get::<CountingGuard>()
        .expect("the counting guard is a provider of the booted app");
    assert_eq!(
        counter.seen.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one root field, three parents: the pool ran once",
    );
}
