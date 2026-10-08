//! `src/allow.rs` reaching the wire — the `Allow` header RFC 9110 §15.5.6
//! requires on a `405`, through a real `#[routes]` mount.

use nest_rs_core::module;
use nest_rs_http::{controller, routes};
use poem::http::{StatusCode, header};

#[controller(path = "/posts")]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    async fn list(&self) -> &'static str {
        "posts"
    }

    #[post("/")]
    async fn create(&self) -> &'static str {
        "created"
    }

    #[delete("/:id")]
    async fn remove(&self) -> &'static str {
        "gone"
    }
}

#[module(providers = [PostsController])]
struct PostsModule;

#[tokio::test]
async fn a_405_names_the_methods_the_route_serves() {
    let client = crate::boot::<PostsModule>().await;
    let resp = client.put("/posts").send().await;
    resp.assert_status(StatusCode::METHOD_NOT_ALLOWED);
    resp.assert_header(header::ALLOW, "GET, HEAD, POST");
}

#[tokio::test]
async fn the_allow_header_survives_the_problem_details_rewrite() {
    let client = crate::boot::<PostsModule>().await;
    let resp = client.patch("/posts/17").send().await;
    resp.assert_status(StatusCode::METHOD_NOT_ALLOWED);
    resp.assert_header(header::ALLOW, "DELETE");
    resp.assert_content_type("application/problem+json");
    let body: serde_json::Value = resp.json().await.value().deserialize();
    assert_eq!(body["status"], 405);
}

// No `#[head]` is declared: poem answers `HEAD` through the `GET` route.
#[tokio::test]
async fn the_advertised_head_answers() {
    let client = crate::boot::<PostsModule>().await;
    client.head("/posts").send().await.assert_status_is_ok();
}

#[controller(path = "/drafts", version = ["1", "2"])]
struct DraftsController;

#[routes]
impl DraftsController {
    #[get("/")]
    async fn list(&self) -> &'static str {
        "drafts"
    }

    #[post("/")]
    #[version("2")]
    async fn create(&self) -> &'static str {
        "created"
    }
}

#[module(providers = [DraftsController])]
struct DraftsModule;

#[tokio::test]
async fn a_version_narrowed_route_advertises_only_what_that_version_serves() {
    let client = crate::boot::<DraftsModule>().await;

    let v1 = client.delete("/v1/drafts").send().await;
    v1.assert_status(StatusCode::METHOD_NOT_ALLOWED);
    v1.assert_header(header::ALLOW, "GET, HEAD");

    let v2 = client.delete("/v2/drafts").send().await;
    v2.assert_status(StatusCode::METHOD_NOT_ALLOWED);
    v2.assert_header(header::ALLOW, "GET, HEAD, POST");
}

#[tokio::test]
async fn an_unrouted_path_is_a_404_with_no_allow() {
    let client = crate::boot::<PostsModule>().await;
    let resp = client.put("/comments").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
    resp.assert_header_is_not_exist(header::ALLOW);
}
