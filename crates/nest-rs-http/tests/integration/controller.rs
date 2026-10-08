//! What `#[routes]` records and what its generated wrapper binds; the handlers
//! reuse the wrapper's local names, so compiling them is half the assertion.

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
    /// `body` is the wrapper's `RequestBody` local, which `ClientIp` reads after it.
    #[post("/body")]
    async fn body_named_body(&self, Json(body): Json<Probe>, ip: ClientIp) -> String {
        format!("{} {}", body.name, ip.ip)
    }

    /// `req` is the wrapper's `Request` local, which `Query` reads after it.
    #[get("/req/:req")]
    async fn param_named_req(&self, Path(req): Path<String>, filter: Query<Filter>) -> String {
        format!("{req} {}", filter.0.limit)
    }

    /// `__ctrl` is the wrapper's `&Arc<Self>` local; `marker` proves the real
    /// instance is called.
    #[get("/ctrl/:value")]
    async fn param_named_ctrl(&self, Path(__ctrl): Path<String>) -> String {
        format!("{} {__ctrl}", self.marker)
    }

    /// A response shaper re-forwards every parameter and binds `__out`,
    /// `__response` and `res` of its own.
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
    // `0.0.0.0` is `ClientIp::unknown()`: the test client has no peer socket.
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

/// Method syntax on the route's `Arc<Controller>` finds this impl before it
/// derefs to the handler of the same name.
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

#[controller(path = "/addresses", version = ["1", "2"])]
struct AddressController;

#[routes]
impl AddressController {
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

    #[get("/parcels/:id")]
    async fn read_parcel(&self, Path(id): Path<String>) -> String {
        format!("read {id}")
    }

    #[delete("parcels/:id/")]
    async fn drop_parcel(&self, Path(id): Path<String>) -> String {
        format!("dropped {id}")
    }

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

// A poem upgrade that reads a path differently from `RoutePath::identity`
// fails here.
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
