//! `#[use_guards]` on a `#[resolver]` impl block — the GraphQL counterpart of a
//! controller's guards. A `ResolverGuard` runs before the operation, reads the
//! per-request state seeded into the context, and allows or denies. Driven
//! end-to-end through the in-process harness.

use nestrs_core::{injectable, module};
use nestrs_graphql::async_graphql::{Context, Error, Result};
use nestrs_graphql::{async_trait, resolver, ContextSeed, GraphqlModule, ResolverGuard};
use nestrs_http::{async_trait as http_async_trait, Guard, HttpTransport};
use nestrs_testing::TestApp;
use poem::http::StatusCode;
use poem::{Request, Response};

/// A per-request role, attached by an HTTP guard from the `x-role` header and
/// forwarded into the GraphQL context — the stand-in for an authenticated caller.
#[derive(Clone)]
struct Role(String);

struct RoleHeaderGuard;

#[http_async_trait]
impl Guard for RoleHeaderGuard {
    async fn check(&self, req: &mut Request) -> std::result::Result<(), Response> {
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

nestrs_graphql::inventory::submit! {
    ContextSeed {
        seed: |req, _container, gql| match req.extensions().get::<Role>() {
            Some(role) => gql.data(role.clone()),
            None => gql,
        },
    }
}

/// The resolver guard: only an `admin` role may proceed. A container-resolved
/// `#[injectable]` provider, exactly like an HTTP guard.
#[injectable]
#[derive(Default)]
struct RequireAdmin;

#[async_trait]
impl ResolverGuard for RequireAdmin {
    async fn check(&self, ctx: &Context<'_>) -> Result<()> {
        match ctx.data_opt::<Role>() {
            Some(role) if role.0 == "admin" => Ok(()),
            _ => Err(Error::new("forbidden")),
        }
    }
}

#[resolver]
struct GuardedResolver;

// Resolver-level guard: every operation runs `RequireAdmin` first. `secret` has no
// `&Context` of its own, so the macro injects one to run the guard against.
#[resolver]
#[use_guards(RequireAdmin)]
impl GuardedResolver {
    #[query]
    async fn secret(&self) -> Result<String> {
        Ok("classified".into())
    }

    /// Already declares `&Context`, so the macro reuses it for the guard rather
    /// than injecting a second one — the path crud-generated ops (which carry
    /// `__ctx`) exercise.
    #[query]
    async fn whoami(&self, ctx: &Context<'_>) -> Result<String> {
        Ok(ctx
            .data_opt::<Role>()
            .map(|r| r.0.clone())
            .unwrap_or_default())
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [RequireAdmin, GuardedResolver])]
struct GuardedModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<GuardedModule>()
        .http(HttpTransport::new().guard(RoleHeaderGuard))
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

    // The guard also runs on an operation that already has its own `&Context`
    // (reuse path), and the body still sees the seeded role.
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
    let app = boot().await;

    // A plain role is rejected by the guard before the body runs.
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

    // No role at all is likewise denied.
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
