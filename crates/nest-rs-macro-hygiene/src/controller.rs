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

use nest_rs::http::poem::web::{Json, Path};
use nest_rs::http::{ClientIp, controller, input, routes};

#[input]
pub struct HygienePayload {
    pub name: String,
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
}
