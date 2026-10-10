//! What a failing handler tells the client, and what it tells the operator: a
//! `DbErr`'s `Display` carries SQL and sometimes row values.
//!
//! ```
//! # use std::sync::Arc;
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_http::poem::Result;
//! # use nest_rs_http::poem::http::StatusCode;
//! # use nest_rs_http::poem::web::{Json, Path};
//! # use nest_rs_http::{Opaque, controller, input, routes};
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
//! let body = reply.0.into_body().into_string().await?;
//! assert!(body.contains(nest_rs_core::OPAQUE_CLIENT_MESSAGE));
//! assert!(!body.contains("does not exist"));
//! # Ok(())
//! # }
//! ```
//!
//! A deliberate error a client can act on (a validation rejection, a `404`) is
//! returned directly, as a `ProblemDetails`.

use nest_rs_core::OPAQUE_CLIENT_MESSAGE;
use poem::Error;

use crate::problem::ProblemDetails;

/// Turn a failure the client must not read into one it may.
///
/// Implemented for every `Result` whose error converts into a boxed error; the
/// whole cause chain reaches the operator's line through
/// [`nest_rs_core::boxed_error`].
pub trait Opaque<T> {
    /// Log the real error for the operator, hand the client an opaque one.
    fn opaque(self) -> Result<T, Error>;
}

impl<T, E> Opaque<T> for Result<T, E>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
{
    fn opaque(self) -> Result<T, Error> {
        self.map_err(|err| {
            let err = nest_rs_core::boxed_error(err);
            tracing::error!(
                target: crate::target::HTTP,
                error = %nest_rs_core::error_message(&*err),
                "request failed",
            );
            // The same envelope as a legible failure, so the shape gives nothing away.
            Error::from(ProblemDetails::internal().with_detail(OPAQUE_CLIENT_MESSAGE))
        })
    }
}

#[cfg(test)]
mod tests {
    use poem::error::ResponseError;
    use poem::http::StatusCode;

    use std::fmt::Display;

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
    fn the_body_carries_the_shared_constant_not_the_error() {
        let out: Result<(), Error> = Err(Leaky).opaque();
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
        let _: Result<(), Error> = Err(Leaky).opaque();

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

    #[test]
    fn it_is_a_500_and_a_problem_document() {
        let out: Result<(), Error> = Err(Leaky).opaque();
        let err = out.expect_err("a failure");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let problem = ProblemDetails::internal().with_detail(OPAQUE_CLIENT_MESSAGE);
        assert_eq!(problem.detail.as_deref(), Some(OPAQUE_CLIENT_MESSAGE));
        assert_eq!(
            problem.as_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn a_success_passes_through_untouched() {
        let out: Result<i32, Error> = Ok::<_, Leaky>(7).opaque();
        assert_eq!(out.ok(), Some(7));
    }
}
