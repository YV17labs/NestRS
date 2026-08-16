//! Request-scoped context: the typed value a guard or interceptor attaches to
//! a request for the handler to read back (e.g. the authenticated principal
//! attached by an auth guard).

use std::any::TypeId;
use std::ops::Deref;

use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

use crate::problem::ProblemDetails;

/// Recorded on a request when an authentication guard evaluated a presented
/// credential, rejected it, and admitted the request anyway because the route
/// is `#[public]`.
///
/// The absorption is deliberate — a public route serves an anonymous caller
/// even when a stale token rides along — but it leaves the request with no
/// principal. A handler that goes on to read one ([`Ctx<Claims>`](Ctx)) then
/// failed with an opaque `500`, turning a forged-credential denial into
/// something indistinguishable from a server bug: no alert, no WAF rule and no
/// rate limit keyed on `401` ever fired.
///
/// Carrying the rejection lets the denial be **deferred** rather than lost.
/// `principal` is the type the guard would have attached, so only a
/// `Ctx<that type>` answers the deferred `401` — any other missing context is
/// still the wiring bug its `500` and `error` log exist to surface.
///
/// Lives here rather than in the kernel: the producer (`nest-rs-authn`) already
/// depends on this crate, and every reader is HTTP-mounted.
#[derive(Clone, Debug)]
pub struct RejectedCredential {
    /// The principal type the rejected credential would have produced.
    pub principal: TypeId,
    /// The client-safe message the guard would have denied with.
    pub client_message: String,
}

/// Extracts a request-scoped value of type `T` an upstream guard or
/// interceptor attached.
///
/// Rejects with `500` if absent — a missing context means the guard that
/// should have set it never ran on this route (a wiring bug, not a client
/// error). `T` is cloned out of the request; store an `Arc<_>` for a large
/// value.
///
/// One exception: when the missing value is missing *because* an
/// authentication guard rejected a presented credential on a `#[public]`
/// route, the deferred `401` is answered instead. Absorbing the rejection is
/// right up to the point a handler needs the principal — past it, a `500`
/// would hide a forged credential behind a server-error shape.
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

impl<'a, T: Clone + Send + Sync + 'static> FromRequest<'a> for Ctx<T> {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        if let Some(value) = req.extensions().get::<T>() {
            return Ok(Ctx(value.clone()));
        }
        // The principal is absent because a credential was presented and
        // rejected on a `#[public]` route. That is a client error, and the
        // guard already logged it at `warn` — answer the deferred 401 so
        // alerting, WAF rules and 401-keyed rate limits all see it. Matched on
        // the principal type: a different missing context is still a wiring
        // bug, and must not be masked as an authentication failure.
        if let Some(rejected) = req
            .extensions()
            .get::<RejectedCredential>()
            .filter(|r| r.principal == TypeId::of::<T>())
        {
            tracing::debug!(
                target: "nest_rs::http",
                context_type = std::any::type_name::<T>(),
                "public route needs the principal a rejected credential never produced — answering the deferred 401",
            );
            return Err(ProblemDetails::unauthorized()
                .with_detail(rejected.client_message.clone())
                .into());
        }
        // Otherwise a missing context is a wiring bug, not a client error. The
        // Rust type name belongs in the logs, not the response body — reply
        // with a bare 500 and record the detail (queryable) at `error`.
        tracing::error!(
            target: "nest_rs::http",
            context_type = std::any::type_name::<T>(),
            "missing request context — the guard or interceptor that sets it did not run on this route",
        );
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Principal(&'static str);

    async fn extract(req: Request) -> Result<Ctx<Principal>> {
        let (req, mut body) = req.split();
        Ctx::<Principal>::from_request(&req, &mut body).await
    }

    #[tokio::test]
    async fn an_attached_principal_extracts() {
        let mut req = Request::default();
        req.extensions_mut().insert(Principal("ada"));
        assert_eq!(extract(req).await.expect("attached").0.0, "ada");
    }

    #[tokio::test]
    async fn a_missing_principal_with_no_rejection_stays_a_wiring_500() {
        let logs = nest_rs_testing::LogCapture::install();
        let err = extract(Request::default())
            .await
            .err()
            .expect("no guard ran");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // The response body is deliberately bare — a Rust type name is not
        // something to hand a client — which makes this line the only record of
        // *which* context was missing. An app attaching several is otherwise
        // told only that one of them did not arrive.
        let event = logs.expect_one(
            "nest_rs::http",
            "missing request context — the guard or interceptor that sets it did not run on this route",
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

    // G5: `#[public]` on an OAuth callback made `AuthnGuard` absorb the
    // forged-`state` denial, so the handler's `Ctx<Caller>` answered 500 —
    // indistinguishable from a server bug, and invisible to any alert or
    // rate limit keyed on 401.
    #[tokio::test]
    async fn a_rejected_credential_turns_the_missing_principal_into_the_deferred_401() {
        let mut req = Request::default();
        req.extensions_mut().insert(RejectedCredential {
            principal: TypeId::of::<Principal>(),
            client_message: "authentication failed".into(),
        });
        let err = extract(req).await.err().expect("credential was rejected");
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        let body = err
            .into_response()
            .into_body()
            .into_bytes()
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["status"], 401);
        assert_eq!(json["detail"], "authentication failed");
    }

    // The deferral is scoped to the principal the guard would have attached.
    // `Ctx<T>` is the generic reader for anything a guard attaches, so an
    // untyped marker made a genuine wiring bug — a domain guard that never ran
    // — answer 401 and swallow the `error` log written to surface it.
    #[tokio::test]
    async fn a_rejected_credential_does_not_mask_an_unrelated_missing_context() {
        #[derive(Clone)]
        struct SomethingElse;

        let mut req = Request::default();
        req.extensions_mut().insert(RejectedCredential {
            principal: TypeId::of::<SomethingElse>(),
            client_message: "authentication failed".into(),
        });
        let err = extract(req).await.err().expect("no guard attached it");
        assert_eq!(
            err.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a different missing context is still a wiring bug",
        );
    }
}
