//! `src/allow.rs` reaching the wire — the `Allow` header RFC 9110 §15.5.6
//! requires on a `405`, through a real `#[routes]` mount.
//!
//! The unit tests build a method table by hand; what they cannot see is the
//! path the verb set actually takes — declared in `#[routes]`, grouped by path,
//! narrowed by `#[version]` at mount time, and rendered by the transport's
//! problem-details boundary, which rebuilds the response body and could drop
//! the header on the way. That whole path is what a `405` answers with.

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

// The header has to survive the transport-edge boundary, which rebuilds a raw
// transport error into `application/problem+json` from scratch. A `405` whose
// `Allow` is dropped by the rewrite satisfies the mount and fails the client.
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

// The advertised `HEAD` is poem's `GET` fallback, so it answers — the set is
// what the router serves, never a superset.
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

// Which verbs a path serves is decided *at mount*, per version, so the
// advertised set has to be decided there too. A set computed from the
// declaration alone would tell a v1 caller to POST at an address that answers
// `405` for POST.
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

// A path nothing claims is still a `404`: the `Allow` set answers for a
// resource that exists, and inventing one for an unrouted path would turn every
// probe into a confirmation that something is there.
#[tokio::test]
async fn an_unrouted_path_is_a_404_with_no_allow() {
    let client = crate::boot::<PostsModule>().await;
    let resp = client.put("/comments").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
    resp.assert_header_is_not_exist(header::ALLOW);
}
