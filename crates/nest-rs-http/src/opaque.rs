//! What a failing handler tells the client, and what it tells the operator: a
//! `DbErr`'s `Display` carries SQL and sometimes row values.
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_http::{Json, Opaque, Path, Result, StatusCode, controller, input, routes};
//! # use nest_rs_testing::TestApp;
//! # #[input]
//! # struct Report {
//! #     title: String,
//! # }
//! # #[injectable]
//! # #[derive(Default)]
//! # struct ReportsService;
//! # impl ReportsService {
//! #     async fn render(&self, _id: u64) -> anyhow::Result<Report> {
//! #         anyhow::bail!("relation reports does not exist")
//! #     }
//! # }
//! # #[controller(path = "/")]
//! # struct ReportsController {
//! #     #[inject]
//! #     svc: Arc<ReportsService>,
//! # }
//! # #[routes]
//! # impl ReportsController {
//! #[get("/reports/{id}")]
//! async fn report(&self, Path(id): Path<u64>) -> Result<Json<Report>> {
//!     Ok(Json(self.svc.render(id).await.opaque()?))
//! }
//! # }
//! # #[module(providers = [ReportsService, ReportsController])]
//! # struct ReportsModule;
//! # #[nest_rs_core::main]
//! # async fn main() -> anyhow::Result<()> {
//! # let app = TestApp::for_module::<ReportsModule>().await?;
//! # let reply = app.http().get("/reports/7").send().await;
//!
//! reply.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
//! let body: serde_json::Value = serde_json::from_str(&reply.0.into_body().into_string().await?)?;
//! assert_eq!(body["code"], "INTERNAL");
//! assert!(body.get("detail").is_none());
//! assert!(!body.to_string().contains("does not exist"));
//! # Ok(())
//! # }
//! ```
//!
//! A deliberate error a client can act on (a validation rejection, a `404`) is
//! returned directly, as a `ProblemDetails`.

use crate::error::HttpError;
use crate::problem::ProblemDetails;

/// Turn a failure the client must not read into one it may.
///
/// Implemented for every `Result` whose error converts into a boxed error; the
/// whole cause chain reaches the operator's line through
/// [`nest_rs_core::boxed_error`].
pub trait Opaque<T> {
    /// Log the real error for the operator, hand the client the opaque `500`:
    /// [`ProblemDetails::internal`], its `code` and no `detail`.
    fn opaque(self) -> Result<T, HttpError>;
}

impl<T, E> Opaque<T> for Result<T, E>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
{
    fn opaque(self) -> Result<T, HttpError> {
        self.map_err(|err| {
            let err = nest_rs_core::boxed_error(err);
            tracing::error!(
                target: crate::target::HTTP,
                error = %nest_rs_core::error_message(&*err),
                "request failed",
            );
            // The document every undescribed server failure answers, so the shape
            // gives nothing away.
            HttpError::from(ProblemDetails::internal())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Display;

    use http::StatusCode;

    use super::*;

    /// An error whose `Display` carries exactly what must not ship.
    #[derive(Debug)]
    struct Leaky;

    impl std::error::Error for Leaky {}

    impl Display for Leaky {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("SELECT password_hash FROM \"user\" WHERE email = 'a@b.test'")
        }
    }

    #[test]
    fn the_error_displays_none_of_its_cause() {
        let out: Result<(), HttpError> = Err(Leaky).opaque();
        let err = out.expect_err("the failure stays a failure");
        let rendered = err.to_string();
        assert!(
            !rendered.contains("password_hash"),
            "the whole point: a `Display` carrying SQL does not reach the wire — got {rendered}",
        );
    }

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test reads the line the call logs, not its result"
    )]
    fn and_the_operator_gets_the_error_the_client_does_not() {
        let logs = nest_rs_testing::LogCapture::install();
        let _: Result<(), HttpError> = Err(Leaky).opaque();

        let event = logs.expect_one("nest_rs::http", "request failed");
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("error")
                .is_some_and(|e| e.contains("password_hash")),
            "the cause the body withholds is exactly what the log has to carry, got {:?}",
            event.fields,
        );
    }

    /// The `500` every opaque failure answers: its class's code, no detail.
    #[tokio::test]
    async fn it_is_the_one_opaque_500_document() {
        let out: Result<(), HttpError> = Err(Leaky).opaque();
        let err = out.expect_err("a failure");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = crate::testing::text(err.into_response()).await;
        let body: serde_json::Value = serde_json::from_str(&body).expect("problem json");
        assert_eq!(body, crate::testing::internal_document());
    }

    #[test]
    fn a_success_passes_through_untouched() {
        let out: Result<i32, HttpError> = Ok::<_, Leaky>(7).opaque();
        assert_eq!(out.ok(), Some(7));
    }
}
