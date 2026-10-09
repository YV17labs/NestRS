//! Covers `src/module.rs` — the documented wiring of each source, what the
//! boot refuses, and where the files sit among routes, the global prefix and
//! the self-mounts: last, outside the prefix, and public.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::{HttpConfig, HttpModule, async_trait, controller, routes};
use nest_rs_openapi::{OpenApiConfig, OpenApiModule};
use nest_rs_static_files::{Embedded, StaticFilesConfig, StaticFilesModule, StaticFilesOptions};
use nest_rs_testing::{LogCapture, TestApp};
use poem::Request;
use poem::http::StatusCode;

use crate::embedded::Site;
use crate::{NAVIGATION, SCRIPT, SITE, files_in, is_problem_404, serve, serve_in, site, text};

#[module(imports = [StaticFilesModule::for_root(StaticFilesConfig {
    root: Some(SITE.into()),
    spa_fallback: true,
    ..Default::default()
})])]
struct DocumentedApp;

#[tokio::test]
async fn the_documented_wiring_serves_a_directory() {
    let app = TestApp::for_module::<DocumentedApp>().await.unwrap();

    let script = app.http().get("/assets/app.js").send().await;
    script.assert_status_is_ok();
    assert_eq!(text(script).await, SCRIPT);
    app.http()
        .get("/settings")
        .header("accept", NAVIGATION)
        .send()
        .await
        .assert_status_is_ok();
}

async fn boot_error<M: nest_rs_core::Module>() -> String {
    match TestApp::for_module::<M>().await {
        Ok(_) => panic!("the boot must refuse these files"),
        Err(err) => format!("{err:#}"),
    }
}

#[module(imports = [StaticFilesModule::for_root(StaticFilesOptions {
    config: Some(StaticFilesConfig { root: Some(SITE.into()), ..Default::default() }),
    embedded: Some(Embedded::of::<Site>()),
})])]
struct RootBesideEmbeddedApp;

#[tokio::test]
async fn a_directory_beside_an_embedded_folder_fails_the_boot() {
    let err = boot_error::<RootBesideEmbeddedApp>().await;
    assert!(
        err.contains(&nest_rs_config::var_name("static_files", "ROOT")),
        "{err}"
    );
    assert!(
        err.contains("StaticFilesConfig::root") && err.contains("Site"),
        "{err}"
    );
}

#[module(imports = [
    StaticFilesModule::for_root(Embedded::of::<Site>()),
    StaticFilesModule::for_root(Embedded::of::<Site>()),
])]
struct TwoEmbeddedApp;

#[tokio::test]
async fn a_second_embedded_folder_fails_the_boot_naming_both_imports() {
    let err = boot_error::<TwoEmbeddedApp>().await;
    assert!(err.contains("contested declaration"), "{err}");
    assert!(
        err.contains("imports[0]") && err.contains("imports[1]"),
        "{err}"
    );
    assert!(err.contains("declare the embedded folder once"), "{err}");
}

#[module(imports = [StaticFilesModule::for_root(StaticFilesConfig {
    root: Some("tests/harness/no-such-directory".into()),
    ..Default::default()
})])]
struct MissingRootApp;

#[module(imports = [StaticFilesModule::for_root(StaticFilesConfig {
    root: Some("tests/harness/site/index.html".into()),
    ..Default::default()
})])]
struct FileRootApp;

#[module(imports = [StaticFilesModule::for_root(None)])]
struct UnsetRootApp;

#[tokio::test]
async fn a_root_that_is_unset_missing_or_a_file_fails_the_boot_naming_the_setting() {
    let root = nest_rs_config::var_name("static_files", "ROOT");
    for (err, says) in [
        (boot_error::<MissingRootApp>().await, "does not resolve"),
        (boot_error::<FileRootApp>().await, "not a directory"),
        (boot_error::<UnsetRootApp>().await, "none is named"),
    ] {
        assert!(err.contains(&root) && err.contains(says), "{err}");
        assert!(!err.contains("no-such-directory"), "never the value: {err}");
    }
}

/// The starter's `GET /`.
#[controller(path = "/")]
struct HomeController;

#[routes]
impl HomeController {
    #[get("/")]
    #[public]
    async fn home(&self) -> &'static str {
        "Hello World"
    }
}

#[controller(path = "/posts")]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    #[public]
    async fn list(&self) -> &'static str {
        "[]"
    }
}

#[module(imports = [StaticFilesModule], providers = [HomeController, PostsController])]
struct BesideRoutesApp;

fn spa() -> StaticFilesConfig {
    StaticFilesConfig {
        spa_fallback: true,
        ..StaticFilesConfig::default()
    }
}

#[tokio::test]
async fn routes_take_precedence_and_one_at_the_index_path_is_reported() {
    let dir = site();
    let logs = LogCapture::install();
    let app = serve_in::<BesideRoutesApp>(dir.path(), spa()).await;

    let warned = logs.expect_one(
        nest_rs_http::target::ROUTES,
        "a route answers GET at the router fallback's own path, which the fallback never answers",
    );
    assert_eq!(warned.level, "warn");
    assert_eq!(
        warned.field("controller").as_deref(),
        Some("HomeController")
    );

    let home = app
        .http()
        .get("/")
        .header("accept", NAVIGATION)
        .send()
        .await;
    home.assert_status_is_ok();
    assert_eq!(text(home).await, "Hello World");
    assert_eq!(text(app.http().get("/posts").send().await).await, "[]");
    assert_eq!(
        text(app.http().get("/assets/app.js").send().await).await,
        SCRIPT
    );
}

#[tokio::test]
async fn a_controller_s_prefix_is_never_a_page() {
    let dir = site();
    let app = serve_in::<BesideRoutesApp>(dir.path(), spa()).await;

    let typo = app
        .http()
        .get("/posts/typo")
        .header("accept", NAVIGATION)
        .send()
        .await;
    assert!(
        is_problem_404(typo).await,
        "the route table's 404, not the app"
    );
}

#[module(
    imports = [
        HttpModule::for_root(HttpConfig {
            global_prefix: Some("/api".into()),
            ..Default::default()
        }),
        StaticFilesModule,
    ],
    providers = [HomeController, PostsController],
)]
struct PrefixedApp;

#[tokio::test]
async fn the_files_sit_outside_the_global_prefix() {
    let dir = site();
    let app = serve_in::<PrefixedApp>(dir.path(), spa()).await;

    assert_eq!(
        text(app.http().get("/api").send().await).await,
        "Hello World"
    );
    assert_eq!(text(app.http().get("/api/posts").send().await).await, "[]");
    let index = app.http().get("/").send().await;
    index.assert_status_is_ok();
    index.assert_content_type("text/html; charset=utf-8");
    for api in ["/api/typo", "/api/posts/typo"] {
        let resp = app
            .http()
            .get(api)
            .header("accept", NAVIGATION)
            .send()
            .await;
        resp.assert_header("x-content-type-options", "nosniff");
        assert!(is_problem_404(resp).await, "{api} is the API's 404");
    }
    app.http()
        .get("/dashboard")
        .header("accept", NAVIGATION)
        .send()
        .await
        .assert_status_is_ok();
}

#[module(imports = [
    OpenApiModule::for_root(OpenApiConfig { enabled: true, ..Default::default() }),
    StaticFilesModule,
])]
struct BesideOpenApiApp;

#[tokio::test]
async fn a_self_mount_keeps_its_subtree() {
    let dir = site();
    let app = serve_in::<BesideOpenApiApp>(dir.path(), spa()).await;

    app.http()
        .get("/api-json")
        .send()
        .await
        .assert_status_is_ok();
    let docs = app
        .http()
        .get("/api")
        .header("accept", NAVIGATION)
        .send()
        .await;
    docs.assert_status_is_ok();
    assert!(!text(docs).await.contains("<title>Publish</title>"));
    let typo = app
        .http()
        .get("/api/typo")
        .header("accept", NAVIGATION)
        .send()
        .await;
    assert!(
        is_problem_404(typo).await,
        "under OpenAPI's path, never the app"
    );
}

#[injectable]
#[derive(Default)]
struct DenyEveryone;

impl Layer for DenyEveryone {}

#[async_trait]
impl Guard for DenyEveryone {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::unauthorized("no"))
    }
}

impl HttpGuard for DenyEveryone {}

#[module(imports = [StaticFilesModule], providers = [PostsController, DenyEveryone])]
struct GuardedApp;

#[tokio::test]
async fn the_files_are_public_whatever_the_global_guards() {
    let dir = site();
    let app = files_in::<GuardedApp>(dir.path(), StaticFilesConfig::default())
        .use_guards_global([guard::<DenyEveryone>()])
        .build()
        .await
        .unwrap();

    app.http()
        .get("/posts")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.http()
        .get("/assets/app.js")
        .send()
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn the_boot_line_names_the_mount_and_the_source() {
    let dir = site();
    let logs = LogCapture::install();
    let _app = serve(dir.path(), StaticFilesConfig::default()).await;

    let line = logs.expect_one(nest_rs_static_files::TARGET, "serving static files");
    assert_eq!(line.level, "info");
    assert_eq!(line.field("path").as_deref(), Some("/"));
    assert_eq!(line.field("source").as_deref(), Some("disk"));
    assert!(line.field("root").is_some());
}
