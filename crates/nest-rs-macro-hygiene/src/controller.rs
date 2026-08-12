//! `#[controller]` + `#[routes]` + the response shapers — the HTTP handler
//! surface, witnessed through the umbrella alone.
//!
//! Two things are proved here that nothing else in the workspace proves:
//!
//! 1. **Path hygiene.** `#[routes]` emits its own `Endpoint` impl rather than
//!    wrapping poem's `#[handler]`, so every path it names is routed through
//!    `::nest_rs_http::poem::…`. A controller crate therefore declares no
//!    `poem` line — this module compiles with `nest-rs` and nothing else.
//! 2. **Identifier hygiene (HTTP-M1).** The wrapper's own locals sit on
//!    `Span::mixed_site()`, so `Json(body)` — the destructure
//!    `/http/extractors/` teaches — cannot mask the `RequestBody` the
//!    extractor after it reads. The behavioural half of that regression lives
//!    in `nest-rs-http/tests/integration/controller.rs`; the half that belongs
//!    here is that it holds through the umbrella's own re-export chain.

use nest_rs::http::poem::web::{Json, Multipart, Path};
use nest_rs::http::{ClientIp, Header, controller, input, routes};

#[input]
pub struct HygienePayload {
    pub name: String,
}

#[input]
pub struct HygieneHeaders {
    #[serde(rename = "X-Hygiene")]
    pub marker: Option<String>,
}

#[input]
pub struct HygieneForm {
    pub file: String,
}

#[controller(path = "/hygiene")]
pub struct HygieneController;

#[routes]
impl HygieneController {
    /// A body parameter under the wrapper's own local name, followed by an
    /// extractor that reads the request — the exact ordering the collision
    /// used to break.
    #[post("/echo")]
    #[public]
    async fn echo(&self, Json(body): Json<HygienePayload>, ip: ClientIp) -> String {
        format!("{} {}", body.name, ip.ip)
    }

    /// The same, under a response shaper: the shaper re-forwards every
    /// parameter through a second emission path.
    #[get("/probe/:req")]
    #[public]
    #[http_code(201)]
    #[response_header("x-hygiene", "ok")]
    async fn probe(&self, Path(req): Path<String>) -> String {
        req
    }

    /// The third response attribute, and the one whose expansion is not a
    /// passthrough: `#[routes]` drains the marker and writes the whole handler
    /// body, so the paths in *that* emission are the ones under test here —
    /// its two siblings above prove nothing about it.
    #[get("/moved")]
    #[public]
    #[redirect("/probe/moved", 308)]
    async fn moved(&self) {}

    /// The OpenAPI facets that are *types* rather than strings: a `Header<T>`
    /// payload and an `#[api(multipart = T)]` form both make the expansion emit
    /// a `schema_of::<T>` and a `RequestBodyMeta`, so both are paths a
    /// controller crate would otherwise have to declare a crate for.
    #[post("/upload")]
    #[public]
    #[api(
        summary = "Upload a form",
        multipart = HygieneForm,
        response_content_type = "text/plain"
    )]
    async fn upload(&self, headers: Header<HygieneHeaders>, form: Multipart) -> String {
        let _ = form;
        headers.into_inner().marker.unwrap_or_default()
    }
}

/// The versioned mount, whose expansion is a different shape again: the routes
/// mount inside a loop over `VERSIONS`, and `#[version]` emits a `const`
/// assertion calling `versions_declare`. Both are framework paths a controller
/// crate would otherwise have to name a crate for.
#[controller(path = "/hygiene-versioned", version = ["1", "2"])]
pub struct HygieneVersionedController;

#[routes]
impl HygieneVersionedController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        "list".into()
    }

    #[post("/")]
    #[public]
    #[version("2")]
    async fn create(&self) -> String {
        "created".into()
    }
}
