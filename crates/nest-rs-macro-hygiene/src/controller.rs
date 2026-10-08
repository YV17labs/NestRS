//! `#[controller]` + `#[routes]` + the response shapers, through the umbrella
//! alone: no `poem` line, and the wrapper's `Span::mixed_site()` locals cannot
//! be masked by a `Json(body)` destructure.

use nest_rs::http::futures_util::stream;
use nest_rs::http::poem::web::{Json, Multipart, Path};
use nest_rs::http::{ClientIp, Header, SseEvent, SseStream, controller, input, routes};

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
    /// A body parameter under the wrapper's own local name, then an extractor
    /// that reads the request.
    #[post("/echo")]
    #[public]
    async fn echo(&self, Json(body): Json<HygienePayload>, ip: ClientIp) -> String {
        format!("{} {}", body.name, ip.ip)
    }

    /// The same, under a response shaper's second emission path.
    #[get("/probe/:req")]
    #[public]
    #[http_code(201)]
    #[response_header("x-hygiene", "ok")]
    async fn probe(&self, Path(req): Path<String>) -> String {
        req
    }

    /// `#[redirect]` is not a passthrough: `#[routes]` writes the whole
    /// handler body, a distinct emission.
    #[get("/moved")]
    #[public]
    #[redirect("/probe/moved", 308)]
    async fn moved(&self) {}

    /// `Header<T>` and `#[api(multipart = T)]` emit a `schema_of::<T>` and a
    /// `RequestBodyMeta`.
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

    /// `#[sse]`: a streaming controller declares neither `futures-util` nor
    /// `poem`.
    #[sse("/events")]
    #[public]
    async fn events(&self) -> SseStream {
        SseStream::new(stream::iter([SseEvent::message("tick")]))
    }

    #[get("/sync")]
    #[public]
    fn sync(&self) -> String {
        "sync".into()
    }

    /// `#[api(deprecated)]` and `#[api(error)]`: a deprecation wrapper and a
    /// declared schema.
    #[get("/legacy")]
    #[public]
    #[api(deprecated = "2026-10-08", error(503 = HygieneHeaders))]
    fn legacy(&self) -> String {
        "legacy".into()
    }

    #[cfg(feature = "seaorm")]
    #[get("/count")]
    #[authorize(nest_rs::authz::Read, crate::entity::Entity)]
    fn count(&self) -> String {
        "0".into()
    }

    /// A route compiled out takes its endpoint, mount, document entry and
    /// guard with it; the paths below do not exist.
    #[cfg(any())]
    #[get("/sync")]
    #[use_guards(crate::does_not_exist::Guard)]
    async fn compiled_out(
        &self,
        body: crate::does_not_exist::Body,
    ) -> crate::does_not_exist::Reply {
        crate::does_not_exist::answer(body)
    }
}

/// The versioned mount: a loop over `VERSIONS` and `#[version]`'s `const`
/// assertion are distinct emissions.
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
