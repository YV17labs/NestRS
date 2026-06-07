//! `HttpConfig.global_prefix` — boot the real `App`, mount two controllers,
//! drive them through `poem::TestClient`, and pin that the prefix is applied
//! exactly once at the root (200 on `/api/<ctrl>`, 404 without it).

use nest_rs_config::{Config, ConfigService};
use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpConfig, HttpTransport, controller, routes};
use poem::test::TestClient;

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/")]
    async fn list_users(&self) -> &'static str {
        "users"
    }
}

#[controller(path = "/orgs")]
struct OrgsController;

#[routes]
impl OrgsController {
    #[get("/")]
    async fn list_orgs(&self) -> &'static str {
        "orgs"
    }
}

#[module(providers = [UsersController, OrgsController])]
struct TwoControllersModule;

async fn boot_with_prefix(
    prefix: Option<&str>,
) -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    let app = App::builder()
        .module::<TwoControllersModule>()
        .build()
        .await
        .expect("module boots");
    let mut transport = HttpTransport::new();
    if let Some(p) = prefix {
        transport = transport.global_prefix(p);
    }
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    let endpoint = transport
        .take_endpoint()
        .expect("configure populates the endpoint");
    TestClient::new(endpoint)
}

#[tokio::test]
async fn global_prefix_serves_controllers_under_the_prefix() {
    let client = boot_with_prefix(Some("/api")).await;

    let users = client.get("/api/users").send().await;
    users.assert_status_is_ok();
    users.assert_text("users").await;

    let orgs = client.get("/api/orgs").send().await;
    orgs.assert_status_is_ok();
    orgs.assert_text("orgs").await;
}

#[tokio::test]
async fn global_prefix_hides_controllers_from_their_unprefixed_paths() {
    let client = boot_with_prefix(Some("/api")).await;

    let users = client.get("/users").send().await;
    assert_eq!(
        users.0.status(),
        poem::http::StatusCode::NOT_FOUND,
        "without the prefix the controller must not be reachable",
    );
    let orgs = client.get("/orgs").send().await;
    assert_eq!(orgs.0.status(), poem::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn global_prefix_none_serves_controllers_at_their_declared_paths() {
    // Smoke test that `global_prefix = None` is a true no-op — the same module
    // serves /users and /orgs without rewriting.
    let client = boot_with_prefix(None).await;

    let users = client.get("/users").send().await;
    users.assert_status_is_ok();
    users.assert_text("users").await;

    let orgs = client.get("/orgs").send().await;
    orgs.assert_status_is_ok();
    orgs.assert_text("orgs").await;
}

#[tokio::test]
async fn global_prefix_normalizes_input_variants() {
    // `"api/"` ⇒ `/api`. Same served paths as the `/api` test — pins that the
    // builder normalization (no leading slash + trailing slash) reaches the
    // mount.
    let client = boot_with_prefix(Some("api/")).await;

    let users = client.get("/api/users").send().await;
    users.assert_status_is_ok();

    let orgs = client.get("/api/orgs").send().await;
    orgs.assert_status_is_ok();
}

#[tokio::test]
async fn global_prefix_root_slash_is_a_noop() {
    // `"/"` collapses to no prefix — the controllers serve at their declared
    // paths, not under `/`.
    let client = boot_with_prefix(Some("/")).await;

    let users = client.get("/users").send().await;
    users.assert_status_is_ok();
}

/// Boot the same controller surface but resolve the prefix through
/// `HttpConfig::from_env` — pins the dual-path rule from the env side.
async fn boot_with_env_config() -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>>
{
    let app = App::builder()
        .module::<TwoControllersModule>()
        .build()
        .await
        .expect("module boots");
    let cfg = HttpConfig::from_env(&ConfigService::for_namespace("http"))
        .expect("HttpConfig::from_env succeeds");
    let mut transport = HttpTransport::new();
    if let Some(prefix) = cfg.global_prefix {
        transport = transport.global_prefix(prefix);
    }
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    let endpoint = transport
        .take_endpoint()
        .expect("configure populates the endpoint");
    TestClient::new(endpoint)
}

#[test]
#[allow(clippy::result_large_err)]
fn global_prefix_is_picked_up_from_nestrs_http_global_prefix_env() {
    // The whole dual-path rule the fix is about — set the env var, let the
    // module-side wiring read it through `HttpConfig::from_env`, observe the
    // controllers under `/api/<x>`. Sync test on purpose: figment::Jail is
    // sync (it scopes env to one thread) so the tokio runtime is opened
    // *inside* the closure.
    figment::Jail::expect_with(|jail| {
        jail.set_env("NESTRS_HTTP__GLOBAL_PREFIX", "/api");
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let client = boot_with_env_config().await;

            let users = client.get("/api/users").send().await;
            users.assert_status_is_ok();
            users.assert_text("users").await;

            let unprefixed = client.get("/users").send().await;
            assert_eq!(
                unprefixed.0.status(),
                poem::http::StatusCode::NOT_FOUND,
                "without the env-set prefix the controller must not be reachable",
            );
        });
        Ok(())
    });
}
