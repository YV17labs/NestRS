//! What a failing handler tells the client, and what it tells the operator —
//! the HTTP half of the seam MCP established and GraphQL and WS joined.
//!
//! HTTP was the last of the four client-facing transports without it, and the
//! omission had no argument behind it: a route's error body is built from
//! whatever the handler's error type prints, and `Display` is the wrong default
//! for that job. A `DbErr` carries SQL, column names and sometimes row values; a
//! storage error carries a bucket key. The framework's own `ServiceError::Db` is
//! `#[error("database error")]` so nothing leaks through it today, but a
//! feature's own error type has no such discipline imposed on it — and a browser
//! is exactly as untrusted as a language model.
//!
//! ```ignore
//! #[get("/reports/:id")]
//! #[authorize(Read, reports::Entity)]
//! async fn report(&self, Path(id): Path<Uuid>) -> Result<Json<Report>> {
//!     Ok(Json(self.svc.render(id).await.opaque()?))
//! }
//! ```
//!
//! **A deliberate error is not this.** A validation rejection, a `Denial`, a
//! `404` a client can act on — those exist to be *read*, and they already carry
//! a wire-safe message. Return them directly; `ProblemDetails` is how.

use std::fmt::Display;

use nest_rs_core::OPAQUE_CLIENT_MESSAGE;
use poem::Error;

use crate::problem::ProblemDetails;

/// Turn a failure the client must not read into one it may.
///
/// Implemented for every `Result` whose error is printable, so it covers a
/// `DbErr`, a storage error, an `anyhow::Error` and a feature's own type without
/// any of them having to know HTTP exists.
///
/// The twin traits on MCP, GraphQL and WS are deliberately separate types rather
/// than one trait generic over the error: the output is what lets `.opaque()?`
/// infer from the enclosing handler's return type. See `nest_rs_core::opaque`.
pub trait Opaque<T> {
    /// Log the real error for the operator, hand the client an opaque one.
    fn opaque(self) -> Result<T, Error>;
}

impl<T, E: Display> Opaque<T> for Result<T, E> {
    fn opaque(self) -> Result<T, Error> {
        self.map_err(|err| {
            tracing::error!(
                target: "nest_rs::http",
                error = %err,
                "request failed",
            );
            // A `ProblemDetails`, not a bare 500: the response shape a client
            // parses must not change because the *reason* is withheld, or an
            // opaque failure becomes distinguishable from a legible one by its
            // envelope alone.
            Error::from(ProblemDetails::internal().with_detail(OPAQUE_CLIENT_MESSAGE))
        })
    }
}

#[cfg(test)]
mod tests {
    use poem::error::ResponseError;
    use poem::http::StatusCode;

    use super::*;

    /// An error whose `Display` carries exactly what must not ship.
    struct Leaky;

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
