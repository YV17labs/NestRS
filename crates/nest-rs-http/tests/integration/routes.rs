//! `src/routes.rs`: the route table the transport assembles from every
//! registrant, and the router it compiles into. Black-box over the public
//! seams, so a router library replacing poem's passes it unchanged.

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{
    BoxEndpoint, HttpConfig, HttpEndpointMeta, HttpRoutes, Method, Request, Response, controller,
    endpoint_fn, routes,
};
use poem::http::{StatusCode, header};

use crate::Client;
use crate::transport::{PATIENCE, connect, read_head, serve_module};

/// The table one self-mount at `/` registers, served under `prefix`; the
/// boot's refusal when the table refuses it.
pub(crate) async fn table_under(
    prefix: Option<&str>,
    mount: impl Fn(&mut HttpRoutes) + Send + Sync + 'static,
) -> Result<Client, String> {
    table_on(
        HttpConfig {
            global_prefix: prefix.map(str::to_owned),
            ..HttpConfig::default()
        },
        mount,
    )
    .await
}

/// The table one self-mount at `/` registers, served on the transport `config`
/// describes; the boot's refusal when the table refuses it.
pub(crate) async fn table_on(
    config: HttpConfig,
    mount: impl Fn(&mut HttpRoutes) + Send + Sync + 'static,
) -> Result<Client, String> {
    let app = App::builder()
        .provide_meta(
            HttpEndpointMeta::new(&crate::PUBLIC_MOUNT, "/", "probe", move |_c, routes| {
                mount(routes)
            })
            .owned_by("Conformance"),
        )
        .build()
        .await
        .expect("the app builds — the table is the transport's");
    let mut transport = crate::built(&config);
    transport
        .configure(app.container())
        .await
        .map_err(|refused| refused.to_string())?;
    Ok(crate::client(
        transport.take_endpoint().expect("configured"),
    ))
}

async fn table(mount: impl Fn(&mut HttpRoutes) + Send + Sync + 'static) -> Client {
    table_under(None, mount)
        .await
        .unwrap_or_else(|refused| panic!("the table boots: {refused}"))
}

/// An endpoint answering `label`, the template the router matched and each
/// named parameter it decoded.
fn echoing(label: &'static str, names: &'static [&'static str]) -> BoxEndpoint {
    BoxEndpoint::new(endpoint_fn(move |req: Request| async move {
        let params: Vec<String> = names
            .iter()
            .map(|name| format!("{name}={}", req.path_param(name).unwrap_or("-")))
            .collect();
        Ok(Response::builder().body(format!(
            "{label} {} {}",
            req.route().unwrap_or("-"),
            params.join(" ")
        )))
    }))
}

async fn get(client: &Client, path: &str) -> (StatusCode, String) {
    let resp = client.get(path).send().await;
    let status = resp.0.status();
    (status, resp.0.into_body().into_string().await.unwrap())
}

#[tokio::test]
async fn static_wins_over_a_parameter_and_a_parameter_over_a_catch_all() {
    let client = table(|routes| {
        routes
            .route(Method::GET, "/files/{*rest}", echoing("rest", &["rest"]))
            .route(Method::GET, "/files/{name}", echoing("name", &["name"]))
            .route(Method::GET, "/files/new", echoing("static", &[]));
    })
    .await;
    assert_eq!(
        get(&client, "/files/new").await,
        (StatusCode::OK, "static /files/new ".into())
    );
    assert_eq!(
        get(&client, "/files/a.txt").await,
        (StatusCode::OK, "name /files/{name} name=a.txt".into())
    );
    assert_eq!(
        get(&client, "/files/a/b.txt").await,
        (StatusCode::OK, "rest /files/{*rest} rest=a/b.txt".into())
    );
}

#[tokio::test]
async fn a_catch_all_never_matches_its_bare_prefix() {
    let client = table(|routes| {
        routes.route(Method::GET, "/assets/{*rest}", echoing("rest", &["rest"]));
    })
    .await;
    assert_eq!(get(&client, "/assets").await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        get(&client, "/assets/app.js").await,
        (StatusCode::OK, "rest /assets/{*rest} rest=app.js".into())
    );
}

#[tokio::test]
async fn a_parameter_is_percent_decoded_once_and_a_slash_stays_inside_it() {
    let client = table(|routes| {
        routes.route(Method::GET, "/files/{name}", echoing("name", &["name"]));
    })
    .await;
    assert_eq!(
        get(&client, "/files/a%2Fb").await,
        (StatusCode::OK, "name /files/{name} name=a/b".into())
    );
    assert_eq!(
        get(&client, "/files/caf%C3%A9%252F").await,
        (StatusCode::OK, "name /files/{name} name=café%2F".into())
    );
}

#[tokio::test]
async fn a_parameter_that_is_not_utf_8_answers_400_without_its_value() {
    let client = table(|routes| {
        routes.route(Method::GET, "/files/{name}", echoing("name", &["name"]));
    })
    .await;
    let resp = client.get("/files/%FF%FE").send().await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    resp.assert_content_type("application/problem+json");
    let body = resp.0.into_body().into_string().await.unwrap();
    assert!(
        !body.contains("%FF") && !body.contains('\u{FFFD}'),
        "{body}"
    );
}

#[tokio::test]
async fn a_parameter_may_follow_literal_text_in_its_segment() {
    let client = table(|routes| {
        routes.route(Method::GET, "/@{handle}", echoing("member", &["handle"]));
    })
    .await;
    assert_eq!(
        get(&client, "/@bob").await,
        (StatusCode::OK, "member /@{handle} handle=bob".into())
    );
    assert_eq!(get(&client, "/bob").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_empty_segment_is_another_path() {
    let client = table(|routes| {
        routes.route(Method::GET, "/a/b", echoing("ab", &[])).route(
            Method::GET,
            "/users/{id}",
            echoing("user", &["id"]),
        );
    })
    .await;
    assert_eq!(get(&client, "/a/b").await.0, StatusCode::OK);
    assert_eq!(get(&client, "/a//b").await.0, StatusCode::NOT_FOUND);
    assert_eq!(get(&client, "/users//").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn two_shapes_of_one_address_fail_the_boot_naming_both() {
    let refused = table_under(None, |routes| {
        routes
            .route(Method::GET, "/parcels/{id}", echoing("read", &[]))
            .route(Method::DELETE, "/parcels/{key}", echoing("release", &[]));
    })
    .await
    .err()
    .expect("one address, two shapes");
    for named in ["Conformance", "\"/parcels/{id}\"", "\"/parcels/{key}\""] {
        assert!(refused.contains(named), "names {named}: {refused}");
    }
}

#[tokio::test]
async fn a_405_carries_allow_and_a_problem_document() {
    let client = table(|routes| {
        routes
            .route(Method::GET, "/posts", echoing("list", &[]))
            .route(Method::POST, "/posts", echoing("create", &[]));
    })
    .await;
    let resp = client.put("/posts").send().await;
    resp.assert_status(StatusCode::METHOD_NOT_ALLOWED);
    resp.assert_header(header::ALLOW, "GET, HEAD, POST");
    resp.assert_content_type("application/problem+json");
    let body: serde_json::Value = resp.json().await.value().deserialize();
    assert_eq!(body["status"], 405);
}

#[tokio::test]
async fn an_entry_for_every_method_dispatches_on_the_method_itself() {
    let client = table(|routes| {
        routes.route_any(
            "/rpc",
            endpoint_fn(|req: Request| async move {
                Ok(Response::builder().body(req.method().as_str().to_owned()))
            }),
        );
    })
    .await;
    for (method, sent) in [
        (Method::PUT, client.put("/rpc")),
        (Method::DELETE, client.delete("/rpc")),
    ] {
        let resp = sent.send().await;
        resp.assert_status_is_ok();
        resp.assert_text(method.as_str()).await;
    }
}

/// Every entry sees the URI a client sent, the global prefix included, and the
/// template it was registered under, prefix included.
#[tokio::test]
async fn an_entry_under_the_global_prefix_sees_the_whole_uri() {
    let client = table_under(Some("/api"), |routes| {
        routes.route(
            Method::GET,
            "/orgs/{org}",
            endpoint_fn(|req: Request| async move {
                Ok(Response::builder().body(format!(
                    "{} {}",
                    req.uri().path(),
                    req.route().unwrap_or("-")
                )))
            }),
        );
    })
    .await
    .unwrap();
    assert_eq!(
        get(&client, "/api/orgs/acme").await,
        (StatusCode::OK, "/api/orgs/acme /api/orgs/{org}".into())
    );
    assert_eq!(get(&client, "/orgs/acme").await.0, StatusCode::NOT_FOUND);
}

#[controller(path = "/greeting")]
struct GreetingController;

#[routes]
impl GreetingController {
    #[get("/")]
    #[public]
    async fn hello(&self) -> &'static str {
        "hello"
    }
}

#[module(providers = [GreetingController])]
struct GreetingModule;

/// RFC 9110 §9.3.2: `HEAD` answers what `GET` would, with no content.
#[tokio::test]
async fn head_on_a_get_route_answers_headers_and_no_body() {
    let client = crate::boot::<GreetingModule>().await;
    let resp = client.head("/greeting").send().await;
    resp.assert_status_is_ok();
    resp.assert_header(header::CONTENT_LENGTH, "5");
    resp.assert_content_type("text/plain; charset=utf-8");
    resp.assert_bytes(b"").await;

    let serving = serve_module::<GreetingModule>(HttpConfig::default()).await;
    let mut socket = connect(serving.port).await;
    tokio::io::AsyncWriteExt::write_all(
        &mut socket,
        b"HEAD /greeting HTTP/1.1\r\nHost: localhost\r\n\r\n\
          GET /greeting HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await
    .expect("sent");
    let head = tokio::time::timeout(PATIENCE, read_head(&mut socket))
        .await
        .expect("answered");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("content-length: 5\r\n"),
        "{head}"
    );
    let next = tokio::time::timeout(PATIENCE, read_head(&mut socket))
        .await
        .expect("the GET behind it is answered: the HEAD wrote no body");
    assert!(next.starts_with("HTTP/1.1 200"), "{next}");
    serving.cancel.cancel();
}

#[tokio::test]
async fn a_self_mount_registering_an_undeclared_path_fails_the_boot_naming_both() {
    let app = App::builder()
        .provide_meta(
            HttpEndpointMeta::new(&crate::PUBLIC_MOUNT, "/tools", "mcp", |_c, routes| {
                routes
                    .route(Method::POST, "/tools", crate::answering("tools"))
                    .route(Method::GET, "/admin", crate::answering("admin"));
            })
            .owned_by("WanderingTools"),
        )
        .build()
        .await
        .expect("the app builds — the refusal is the transport's");
    let msg = crate::built(&HttpConfig::default())
        .configure(app.container())
        .await
        .expect_err("a path the mount did not declare must fail the boot")
        .to_string();
    assert!(
        msg.contains("WanderingTools") && msg.contains("\"/admin\"") && msg.contains("\"/tools\""),
        "names the owner, the path and what it declared: {msg}"
    );
}
