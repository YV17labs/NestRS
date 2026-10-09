//! The router's fallback answers only what nothing else claims: routes and
//! every self-mount take precedence, a prefix someone owns is never its, it
//! sits outside the global prefix, and it is handed the request whole.

use nest_rs_core::{App, ContainerBuilder, Discoverable, Transport, module};
use nest_rs_http::{
    ApiVersioning, DEFAULT_VERSION_HEADER, HttpEndpointMeta, HttpFallbackMeta, HttpTransport,
    VersionSelector, controller, routes,
};
use nest_rs_testing::LogCapture;
use poem::http::{HeaderName, StatusCode};
use poem::test::TestClient;
use poem::{Request, Route, handler};

#[controller(path = "/")]
struct HomeController;

#[routes]
impl HomeController {
    #[get("/")]
    async fn home(&self) -> &'static str {
        "home"
    }
}

#[controller(path = "/posts")]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    async fn list(&self) -> &'static str {
        "posts"
    }

    #[get("/:id")]
    async fn one(&self) -> poem::Result<&'static str> {
        Err(poem::error::NotFoundError.into())
    }
}

/// Echoes what the fallback was handed: the request line, a header, the body.
#[handler]
fn echo(req: &Request, body: String) -> String {
    format!(
        "fallback {} {} {} {body}",
        req.method(),
        req.uri().path(),
        req.header("x-probe").unwrap_or("-"),
    )
}

fn fallback_at(path: &'static str, owner: &'static str) -> HttpFallbackMeta {
    HttpFallbackMeta::new(path, owner, move |_, route: Route| {
        route
            .at(path, echo)
            .at(nest_rs_http::join_path(path, "*rest"), echo)
    })
}

struct RootFallback;

impl Discoverable for RootFallback {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<Self, HttpFallbackMeta>(fallback_at("/", "FilesHost"))
    }
}

struct AssetsFallback;

impl Discoverable for AssetsFallback {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<Self, HttpFallbackMeta>(fallback_at("/assets", "AssetsHost"))
    }
}

/// A self-mount owning its path, as OpenAPI's `/api` does.
struct DocsMount;

impl Discoverable for DocsMount {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<Self, HttpEndpointMeta>(
            HttpEndpointMeta::new("/docs", "docs", |_, route: Route| {
                route.at("/docs", poem::endpoint::make_sync(|_| "docs"))
            })
            .owned_by("DocsHost")
            .exempt(),
        )
    }
}

struct DocsFallback;

impl Discoverable for DocsFallback {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<Self, HttpFallbackMeta>(fallback_at("/docs/files", "DocsFiles"))
    }
}

#[module(providers = [HomeController, PostsController, DocsMount, RootFallback])]
struct AppModule;

#[module(providers = [RootFallback, AssetsFallback])]
struct TwoFallbacksModule;

#[module(providers = [DocsMount, DocsFallback])]
struct FallbackUnderAMountModule;

type Client = TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>>;

async fn text(client: &Client, path: &str) -> (StatusCode, String) {
    let resp = client.get(path).header("x-probe", "kept").send().await;
    let status = resp.0.status();
    (status, resp.0.into_body().into_string().await.unwrap())
}

#[tokio::test]
async fn routes_take_precedence_and_the_fallback_answers_the_rest() {
    let logs = LogCapture::install();
    let client = crate::boot::<AppModule>().await;

    let warned = logs.expect_one(
        nest_rs_http::target::ROUTES,
        "a route answers GET at the router fallback's own path, which the fallback never answers",
    );
    assert_eq!(warned.level, "warn");
    assert_eq!(
        warned.field("controller").as_deref(),
        Some("HomeController")
    );

    assert_eq!(text(&client, "/").await, (StatusCode::OK, "home".into()));
    assert_eq!(
        text(&client, "/posts").await,
        (StatusCode::OK, "posts".into())
    );
    assert_eq!(
        text(&client, "/docs").await,
        (StatusCode::OK, "docs".into())
    );
    assert_eq!(
        text(&client, "/assets/logo.svg").await,
        (StatusCode::OK, "fallback GET /assets/logo.svg kept ".into()),
    );
}

#[tokio::test]
async fn the_fallback_is_handed_the_request_whole() {
    let client = crate::boot::<AppModule>().await;

    let resp = client
        .post("/upload")
        .header("x-probe", "kept")
        .body("the body")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("fallback POST /upload kept the body")
        .await;
}

#[tokio::test]
async fn a_route_s_own_404_and_an_owned_prefix_never_reach_the_fallback() {
    let client = crate::boot::<AppModule>().await;

    for owned in ["/posts/42", "/posts/42/comments", "/docs/typo"] {
        let (status, body) = text(&client, owned).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{owned}: {body}");
        assert!(
            !body.contains("fallback"),
            "{owned} is its owner's 404: {body}"
        );
    }
}

#[tokio::test]
async fn the_fallback_sits_outside_the_global_prefix() {
    let client = crate::boot_on::<AppModule>(HttpTransport::new().global_prefix("/api")).await;

    assert_eq!(text(&client, "/api").await, (StatusCode::OK, "home".into()));
    assert_eq!(
        text(&client, "/").await,
        (StatusCode::OK, "fallback GET / kept ".into()),
        "the prefix moved the routes, never the fallback",
    );
    let (status, body) = text(&client, "/api/typo").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        !body.contains("fallback"),
        "the prefix is the API's: {body}"
    );
    assert_eq!(
        text(&client, "/apidocs").await,
        (StatusCode::OK, "fallback GET /apidocs kept ".into()),
        "a shared spelling is not the prefix's segment",
    );
}

#[tokio::test]
async fn a_version_selector_in_front_still_leaves_the_rest_to_the_fallback() {
    #[controller(path = "/items", version = "1")]
    struct ItemsController;

    #[routes]
    impl ItemsController {
        #[get("/")]
        async fn list(&self) -> &'static str {
            "items v1"
        }
    }

    #[module(providers = [ItemsController, RootFallback])]
    struct VersionedModule;

    let selector = VersionSelector::new(
        ApiVersioning::Header,
        HeaderName::from_static(DEFAULT_VERSION_HEADER),
        Some("1".into()),
    );
    let client =
        crate::boot_on::<VersionedModule>(HttpTransport::new().api_versioning(selector)).await;

    assert_eq!(
        text(&client, "/items").await,
        (StatusCode::OK, "items v1".into())
    );
    assert_eq!(
        text(&client, "/favicon.ico").await,
        (StatusCode::OK, "fallback GET /favicon.ico kept ".into()),
    );
    let (status, _) = text(&client, "/items/typo").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "under the controller's prefix"
    );
}

async fn configure_error<M: nest_rs_core::Module>() -> String {
    let app = App::builder()
        .module::<M>()
        .build()
        .await
        .expect("the module builds — the refusal is the transport's");
    HttpTransport::new()
        .configure(app.container())
        .await
        .expect_err("the transport must refuse this fallback")
        .to_string()
}

#[tokio::test]
async fn two_fallbacks_fail_the_boot_naming_both() {
    let msg = configure_error::<TwoFallbacksModule>().await;
    assert!(msg.contains("two router fallbacks"), "{msg}");
    assert!(
        msg.contains("FilesHost") && msg.contains("AssetsHost"),
        "{msg}"
    );
}

#[tokio::test]
async fn a_fallback_under_another_owner_s_path_fails_the_boot_naming_it() {
    let msg = configure_error::<FallbackUnderAMountModule>().await;
    assert!(
        msg.contains("\"/docs/files\"") && msg.contains("DocsHost"),
        "{msg}"
    );
}

#[tokio::test]
async fn a_fallback_under_the_global_prefix_fails_the_boot_naming_the_setting() {
    #[module(providers = [AssetsFallback])]
    struct AssetsModule;

    let app = App::builder()
        .module::<AssetsModule>()
        .build()
        .await
        .expect("the module builds");
    let msg = HttpTransport::new()
        .global_prefix("/assets")
        .configure(app.container())
        .await
        .expect_err("the global prefix is the API's")
        .to_string();
    assert!(
        msg.contains(&nest_rs_config::var_name("http", "GLOBAL_PREFIX")),
        "{msg}"
    );
}
