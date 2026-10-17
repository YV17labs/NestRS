//! Request-scoped context: the typed value a guard attaches to the request's
//! unit for the handler to read back (e.g. the authenticated principal an
//! authentication guard attaches).

use std::any::TypeId;
use std::ops::Deref;

use http::{Extensions, StatusCode};
use nest_rs_core::AmbientValue;

use crate::problem::ProblemDetails;
use crate::response::{IntoResponse, Response};

/// Recorded on a request when an authentication guard evaluated a presented
/// credential, rejected it, and admitted the request anyway because the route
/// is `#[public]`.
///
/// A [`Ctx`] of the `principal` type then answers the deferred `401` instead of
/// a `500`; any other missing context stays a wiring bug.
#[derive(Clone, Debug)]
pub struct RejectedCredential {
    /// The principal type the rejected credential would have produced.
    pub principal: TypeId,
    /// The client-safe message the guard would have denied with.
    pub client_message: String,
    /// The RFC 6750 §3.1 error code this rejection reports on the
    /// `WWW-Authenticate` challenge, or `None` when §3 says to report none —
    /// which is the case for a request that carried no credentials at all.
    pub bearer_error: Option<&'static str>,
}

/// Extracts a value of type `T` a guard attached to the request with
/// `UnitContext::attach` — ambient for the rest of the request once the guards
/// passed.
///
/// Rejects with `500` if absent — the guard that should have attached it never
/// ran on this route. `T` is cloned out of the ambient context; attach a value
/// that is cheap to clone.
///
/// One exception: when an authentication guard rejected a presented credential
/// on a `#[public]` route, the deferred `401` ([`RejectedCredential`]) answers.
pub struct Ctx<T>(pub T);

impl<T> Ctx<T> {
    /// Take ownership of the extracted value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Ctx<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Why no guard's value is there to read.
enum Absent<'a> {
    /// A `#[public]` route admitted a credential it refused: the deferred `401`.
    Deferred(&'a RejectedCredential),
    /// The guard that attaches the value never ran: a wiring `500`.
    Unwired,
}

impl RejectedCredential {
    /// The deferred `401`: the guard's client-safe message, and the RFC 6750
    /// challenge it would have stamped.
    fn deferred(&self) -> Response {
        let mut response = ProblemDetails::unauthorized()
            .with_detail(self.client_message.clone())
            .into_response();
        if let Some(code) = self.bearer_error {
            crate::challenge::stamp_bearer_error_on(response.headers_mut(), code);
        }
        response
    }
}

impl<T: AmbientValue + Clone> Ctx<T> {
    /// What a guard attached, or why it is absent.
    fn read(extensions: &Extensions) -> Result<Self, Absent<'_>> {
        if let Some(value) = nest_rs_core::ambient::<T>() {
            return Ok(Ctx(T::clone(&value)));
        }
        if let Some(rejected) = extensions
            .get::<RejectedCredential>()
            .filter(|r| r.principal == TypeId::of::<T>())
        {
            tracing::debug!(
                target: crate::target::HTTP,
                context_type = std::any::type_name::<T>(),
                "public route needs the principal a rejected credential never produced — answering the deferred 401",
            );
            return Err(Absent::Deferred(rejected));
        }
        // The type name belongs in the log, never the response body.
        tracing::error!(
            target: crate::target::HTTP,
            context_type = std::any::type_name::<T>(),
            "missing request context — the guard that attaches it did not run on this route",
        );
        Err(Absent::Unwired)
    }
}

impl<'a, T: AmbientValue + Clone> crate::FromRequest<'a> for Ctx<T> {
    async fn from_request(
        req: &'a crate::Request,
        _body: &mut crate::RequestBody,
    ) -> crate::Result<Self> {
        Self::read(req.extensions()).map_err(|absent| match absent {
            Absent::Deferred(rejected) => crate::HttpError::from_response(rejected.deferred()),
            Absent::Unwired => crate::HttpError::from_status(StatusCode::INTERNAL_SERVER_ERROR),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FromRequest, Request, Result};

    #[derive(Clone)]
    struct Principal(&'static str);

    impl AmbientValue for Principal {}

    async fn extract(req: Request) -> Result<Ctx<Principal>> {
        let (req, mut body) = req.split();
        Ctx::<Principal>::from_request(&req, &mut body).await
    }

    #[tokio::test]
    async fn an_attached_principal_extracts() {
        let extracted = nest_rs_core::with_ambient(
            std::sync::Arc::new(Principal("ada")),
            extract(Request::builder().finish()),
        )
        .await;
        assert_eq!(extracted.expect("attached").0.0, "ada");
    }

    #[tokio::test]
    async fn a_value_in_the_requests_extensions_is_not_an_attachment() {
        let mut req = Request::builder().finish();
        req.extensions_mut().insert(Principal("mallory"));
        let err = extract(req).await.err().expect("nothing was attached");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn a_missing_principal_with_no_rejection_stays_a_wiring_500() {
        let logs = nest_rs_testing::LogCapture::install();
        let err = extract(Request::builder().finish())
            .await
            .err()
            .expect("no guard ran");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let event = logs.expect_one(
            "nest_rs::http",
            "missing request context — the guard that attaches it did not run on this route",
        );
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("context_type")
                .is_some_and(|t| t.contains("Principal")),
            "the event names the context type that never arrived, got {:?}",
            event.fields,
        );
    }

    #[tokio::test]
    async fn a_rejected_credential_turns_the_missing_principal_into_the_deferred_401() {
        let mut req = Request::builder().finish();
        req.extensions_mut().insert(RejectedCredential {
            principal: TypeId::of::<Principal>(),
            client_message: "authentication failed".into(),
            bearer_error: Some(crate::challenge::INVALID_TOKEN),
        });
        let err = extract(req).await.err().expect("credential was rejected");
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        let response = err.into_response();
        // RFC 6750 §3: a Bearer `401` carries the challenge with §3.1's code.
        assert_eq!(
            response
                .headers()
                .get(http::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some(r#"Bearer error="invalid_token""#),
        );
        let body = response.into_body().into_bytes().await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["status"], 401);
        assert_eq!(json["detail"], "authentication failed");
    }

    /// RFC 6750 §3: a request carrying no credentials gets no `error` code; RFC
    /// 9110 §11.6.1 still requires the bare challenge.
    #[tokio::test]
    async fn a_rejection_with_no_code_defers_a_401_whose_challenge_names_none() {
        let mut req = Request::builder().finish();
        req.extensions_mut().insert(RejectedCredential {
            principal: TypeId::of::<Principal>(),
            client_message: "authentication failed".into(),
            bearer_error: None,
        });
        let err = extract(req).await.err().expect("credential was rejected");
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        let challenge = err
            .into_response()
            .headers()
            .get(http::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .expect("a 401 always carries a challenge");
        assert_eq!(challenge, "Bearer");
        assert!(!challenge.contains("error="));
    }

    #[tokio::test]
    async fn a_rejected_credential_does_not_mask_an_unrelated_missing_context() {
        #[derive(Clone)]
        struct SomethingElse;

        impl AmbientValue for SomethingElse {}

        let mut req = Request::builder().finish();
        req.extensions_mut().insert(RejectedCredential {
            principal: TypeId::of::<SomethingElse>(),
            client_message: "authentication failed".into(),
            bearer_error: Some(crate::challenge::INVALID_TOKEN),
        });
        let err = extract(req).await.err().expect("no guard attached it");
        assert_eq!(
            err.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a different missing context is still a wiring bug",
        );
    }
}
