//! What `#[routes]` records and what its generated wrapper binds — the two
//! halves of `src/controller.rs`.
//!
//! **Parameter-name hygiene (HTTP-M1).** The wrapper binds locals of its own
//! (`req`, `body`, `res`, the controller `Arc`, plus three more from the
//! response shapers) around the extractors it emits for the developer's
//! parameters. When those shared one namespace, a parameter
//! spelled `body` — which is what `Json(body): Json<T>`, the idiom
//! `/http/extractors/` teaches, normalizes to — masked the `RequestBody` every
//! *later* extractor reads, and the mismatched-type error landed on the
//! `#[routes]` attribute naming neither the parameter nor the collision.
//!
//! Compiling this module is most of the assertion; the requests below add the
//! other half, that each handler still receives the value it declared rather
//! than one of the wrapper's locals under the same name.

use nest_rs_core::module;
use nest_rs_http::{ClientIp, controller, routes};
use poem::test::TestClient;
use poem::web::{Json, Path, Query};
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Probe {
    name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Filter {
    limit: u32,
}

#[controller(path = "/hygiene")]
struct HygieneController {
    marker: &'static str,
}

impl Default for HygieneController {
    fn default() -> Self {
        Self {
            marker: "controller",
        }
    }
}

#[routes]
impl HygieneController {
    /// `body` — the wrapper's `RequestBody` local, and the name the documented
    /// `Json(body)` destructure normalizes to. `ClientIp` extracts *after* it,
    /// so a masked binding fails to compile.
    #[post("/body")]
    async fn body_named_body(&self, Json(body): Json<Probe>, ip: ClientIp) -> String {
        format!("{} {}", body.name, ip.ip)
    }

    /// `req` — the wrapper's `Request` local. The `Query` extractor after it
    /// reads the request, so a masked binding fails to compile.
    #[get("/req/:req")]
    async fn param_named_req(&self, Path(req): Path<String>, filter: Query<Filter>) -> String {
        format!("{req} {}", filter.0.limit)
    }

    /// `__ctrl` — the local holding `&Arc<Self>`. A shadowed one would forward
    /// the controller where the handler declared a `Path`, so the marker read
    /// below is what proves the call still targets the real instance.
    #[get("/ctrl/:value")]
    async fn param_named_ctrl(&self, Path(__ctrl): Path<String>) -> String {
        format!("{} {__ctrl}", self.marker)
    }

    /// The same collision under a response shaper, which re-forwards every
    /// parameter through a second code path (`apply_response_shapers`) — and
    /// binds three locals of its own, named here too.
    #[post("/shaped/:tag")]
    #[http_code(201)]
    #[response_header("x-hygiene", "ok")]
    async fn shaped(
        &self,
        Json(__out): Json<Probe>,
        Path(__response): Path<String>,
        res: ClientIp,
    ) -> String {
        format!("{} {__response} {}", __out.name, res.ip)
    }
}

#[module(providers = [HygieneController])]
struct HygieneModule;

async fn boot() -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    crate::boot::<HygieneModule>().await
}

#[tokio::test]
async fn a_parameter_named_body_reaches_the_handler_and_leaves_later_extractors_intact() {
    let client = boot().await;
    let resp = client
        .post("/hygiene/body")
        .body_json(&serde_json::json!({ "name": "probe" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    // `0.0.0.0` is `ClientIp::unknown()` — the test client has no peer socket.
    // What matters is that the extractor ran at all.
    resp.assert_text("probe 0.0.0.0").await;
}

#[tokio::test]
async fn a_parameter_named_req_reaches_the_handler_and_leaves_later_extractors_intact() {
    let client = boot().await;
    let resp = client
        .get("/hygiene/req/alpha")
        .query("limit", &"7")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("alpha 7").await;
}

#[tokio::test]
async fn a_parameter_named_ctrl_does_not_displace_the_controller_instance() {
    let client = boot().await;
    let resp = client.get("/hygiene/ctrl/beta").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("controller beta").await;
}

#[tokio::test]
async fn the_collision_stays_closed_under_a_response_shaper() {
    let client = boot().await;
    let resp = client
        .post("/hygiene/shaped/tagged")
        .body_json(&serde_json::json!({ "name": "shaped" }))
        .send()
        .await;
    resp.assert_status(poem::http::StatusCode::CREATED);
    resp.assert_header("x-hygiene", "ok");
    resp.assert_text("shaped tagged 0.0.0.0").await;
}

/// A trait whose method shares a handler's name, implemented for the `Arc` the
/// route holds its controller in. Method syntax on that `Arc` finds this
/// implementation before it derefs to the controller, so a route calling
/// `__ctrl.named_like_a_trait()` ran this body instead of the handler.
#[expect(
    dead_code,
    reason = "never called: the expansion calls the method by its path"
)]
trait NamedLikeATrait {
    fn named_like_a_trait(&self) -> String;
}

impl<T> NamedLikeATrait for std::sync::Arc<T> {
    fn named_like_a_trait(&self) -> String {
        "the trait on Arc".into()
    }
}

#[controller(path = "/shadowed")]
#[derive(Default)]
struct ShadowedController;

#[routes]
impl ShadowedController {
    #[get("/")]
    fn named_like_a_trait(&self) -> String {
        "the handler".into()
    }
}

#[module(providers = [ShadowedController])]
struct ShadowedModule;

#[tokio::test]
async fn a_route_calls_its_handler_and_not_a_trait_method_of_the_same_name_on_arc() {
    let client = crate::boot::<ShadowedModule>().await;
    let resp = client.get("/shadowed").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("the handler").await;
}

// ---------------------------------------------------------------------------
// A route's identity is the address poem mounts, read with poem's grammar
// (`nest_rs_codegen::RoutePath`), and it is claimed in the versions the route
// serves. The refusals are trybuild snapshots; what is pinned here is what the
// same reading *serves*.

#[controller(path = "/addresses", version = ["1", "2"])]
struct AddressController;

#[routes]
impl AddressController {
    /// One verb and path in two versions is two addresses, `/v1/…` and `/v2/…`.
    #[get("/versioned")]
    #[version("1")]
    async fn versioned_one(&self) -> String {
        "one".into()
    }

    #[get("/versioned")]
    #[version("2")]
    async fn versioned_two(&self) -> String {
        "two".into()
    }

    /// Two spellings of one address, two verbs: one poem node answering both.
    #[get("/parcels/:id")]
    async fn read_parcel(&self, Path(id): Path<String>) -> String {
        format!("read {id}")
    }

    #[delete("parcels/:id/")]
    async fn drop_parcel(&self, Path(id): Path<String>) -> String {
        format!("dropped {id}")
    }

    /// Declared with the trailing slash the edge trims off every request.
    #[get("/slashed/")]
    async fn slashed(&self) -> String {
        "slashed".into()
    }
}

#[module(providers = [AddressController])]
struct AddressModule;

#[tokio::test]
async fn one_route_in_two_versions_serves_each_version_with_its_own_handler() {
    let client = crate::boot::<AddressModule>().await;
    for (path, body) in [
        ("/v1/addresses/versioned", "one"),
        ("/v2/addresses/versioned", "two"),
    ] {
        let resp = client.get(path).send().await;
        resp.assert_status_is_ok();
        resp.assert_text(body).await;
    }
}

#[tokio::test]
async fn two_spellings_of_one_address_mount_one_node_serving_both_verbs() {
    let client = crate::boot::<AddressModule>().await;
    let read = client.get("/v1/addresses/parcels/7").send().await;
    read.assert_status_is_ok();
    read.assert_text("read 7").await;
    let dropped = client.delete("/v1/addresses/parcels/7").send().await;
    dropped.assert_status_is_ok();
    dropped.assert_text("dropped 7").await;
}

#[tokio::test]
async fn a_route_declared_with_a_trailing_slash_is_served_at_the_address_without_it() {
    let client = crate::boot::<AddressModule>().await;
    let resp = client.get("/v2/addresses/slashed").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("slashed").await;
}

/// The reading is pinned against poem itself, pair by pair: two paths share a
/// [`RoutePath::identity`] exactly when poem, behind the edge's trailing-slash
/// trim, serves them as one address — refusing the second mount, or answering
/// both probes with one endpoint. A poem upgrade that reads a path differently
/// fails here.
#[tokio::test]
async fn a_route_identity_is_the_address_poem_serves() {
    use nest_rs_codegen::RoutePath;
    use poem::{Route, endpoint::make_sync};

    /// A path and a request it serves.
    type Probe = (&'static str, &'static str);
    // Two probes, and whether the macro calls them one address.
    let pairs: &[(Probe, Probe, bool)] = &[
        (("/t", "/t"), ("t", "/t"), true),
        (("/u", "/u"), ("/u/", "/u"), true),
        (("/a//b", "/a/b"), ("/a/b", "/a/b"), true),
        (("/q/:id", "/q/7"), ("/q/:other", "/q/8"), true),
        (("/n/:id<\\d+>", "/n/7"), ("/n/<\\d+>", "/n/8"), true),
        (("/f/*", "/f/x/y"), ("/f/*rest", "/f/z"), true),
        (("/Users", "/Users"), ("/users", "/users"), false),
        (("/q/:id", "/q/7"), ("/q/:id/x", "/q/7/x"), false),
        (("/n/:id", "/n/abc"), ("/n/:id<\\d+>", "/n/7"), false),
        (("/r/<\\d+>", "/r/7"), ("/r/<[a-z]+>", "/r/abc"), false),
        (("/c/*", "/c/x/y"), ("/c/:id", "/c/x"), false),
    ];

    for ((a, probe_a), (b, probe_b), same) in pairs {
        let identity = |path: &str| {
            RoutePath::parse(path)
                .unwrap_or_else(|why| panic!("`{path}`: {why}"))
                .identity()
                .to_owned()
        };
        assert_eq!(
            identity(a) == identity(b),
            *same,
            "the macro's reading of `{a}` and `{b}`",
        );

        let mounted = Route::new()
            .try_at(*a, make_sync(|_| "a"))
            .and_then(|route| route.try_at(*b, make_sync(|_| "b")));
        let poem_says_one = match mounted {
            Err(_) => true,
            Ok(route) => {
                let client = TestClient::new(route);
                let mut answers = Vec::new();
                for probe in [probe_a, probe_b] {
                    // The edge trims a trailing slash before poem routes.
                    let trimmed = match probe.trim_end_matches('/') {
                        "" => "/",
                        trimmed => trimmed,
                    };
                    let resp = client.get(trimmed).send().await;
                    resp.assert_status_is_ok();
                    answers.push(resp.0.into_body().into_string().await.unwrap_or_default());
                }
                answers[0] == answers[1]
            }
        };
        assert_eq!(poem_says_one, *same, "poem's reading of `{a}` and `{b}`");
    }
}
