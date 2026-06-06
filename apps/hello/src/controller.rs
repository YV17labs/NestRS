use std::sync::Arc;

use nest_rs_http::{controller, routes};
use poem::error::ResponseError;
use poem::http::{HeaderValue, StatusCode, header};
use poem::{IntoResponse, Response};

use crate::service::HelloService;

#[controller(path = "/")]
pub struct HelloController {
    #[inject]
    svc: Arc<HelloService>,
}

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> String {
        self.svc.greeting()
    }

    #[post("/echo")]
    #[http_code(201)]
    #[response_header("x-powered-by", "nestrs")]
    async fn echo(&self) -> String {
        self.svc.greeting()
    }

    #[get("/docs")]
    #[redirect("https://docs.nestrs.dev", 301)]
    #[allow(dead_code, reason = "body discarded by #[redirect]")]
    async fn docs(&self) {}

    /// Proves Bug 1 / 5: the `Err` arm keeps its 403 (from `ResponseError`)
    /// instead of being silently rewritten to 201, and the Result-returning
    /// signature compiles even though `Result<T, E>` is not itself
    /// `IntoResponse`.
    #[post("/forbidden")]
    #[http_code(201)]
    async fn forbidden(&self) -> Result<String, ForbiddenError> {
        Err(ForbiddenError)
    }

    /// Proves Bug 11: the decorator-set `content-type` overrides the
    /// handler-set one, so the response carries a single `content-type`
    /// header (was duplicated before the `insert()` switch).
    #[get("/xml-as-json")]
    #[response_header("content-type", "application/json")]
    async fn xml_as_json(&self) -> Response {
        let mut resp = "<root/>".into_response();
        resp.headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/xml"));
        resp
    }
}

#[derive(Debug)]
pub struct ForbiddenError;

impl std::fmt::Display for ForbiddenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("forbidden")
    }
}

impl std::error::Error for ForbiddenError {}

impl ResponseError for ForbiddenError {
    fn status(&self) -> StatusCode {
        StatusCode::FORBIDDEN
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::Discoverable;

    use super::HelloController;
    use crate::service::HelloService;

    #[test]
    fn controller_declares_its_injected_dependency_for_the_access_graph() {
        assert!(HelloController::dependencies().is_empty());
        assert!(
            HelloController::injected().contains(&TypeId::of::<HelloService>()),
            "the controller's injected HelloService is recorded for the access graph",
        );
    }
}
