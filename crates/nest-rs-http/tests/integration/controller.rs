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
