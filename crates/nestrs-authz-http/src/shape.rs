//! Two transparent jobs `Authorize<A, S>` does as a [`RouteResponseShaper`], both
//! keyed on the caller's ability that the guard attached:
//!
//! 1. **Ambient ability** — [`run`](RouteResponseShaper::run) wraps the handler in
//!    [`with_ability`], so the data layer (`nestrs-database`'s `Repo`) reads the
//!    caller's ability via `current_ability()` and scopes every query — the
//!    developer writes no filter.
//! 2. **Response masking** — after the handler, the body is parsed back into
//!    `S::Model` and run through the same typed [`Ability::mask`]/
//!    [`Ability::mask_many`] the engine already uses, dropping the rows and fields
//!    the ability does not permit — no `mask` call in the handler, and no second
//!    masking implementation to keep in step.
//!
//! Masking is a security control, so this fails *closed*: a successful JSON body
//! that does not deserialize into `S::Model` (a handler/subject mismatch) yields
//! a `500` rather than shipping the data unmasked. The whole body is buffered to
//! mask it, so masked list endpoints should be paginated.

use std::future::Future;
use std::sync::Arc;

use nestrs_http::RouteResponseShaper;
use poem::http::StatusCode;
use poem::{Request, Response, Result};
use sea_orm::EntityTrait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use nestrs_authz::{with_ability, Ability, Action, ActionMarker};

use crate::extractor::Authorize;

impl<A, S> RouteResponseShaper for Authorize<A, S>
where
    A: ActionMarker,
    S: EntityTrait,
    S::Model: DeserializeOwned + Serialize,
{
    type Captured = Option<Arc<Ability>>;

    fn capture(req: &Request) -> Self::Captured {
        req.extensions().get::<Arc<Ability>>().cloned()
    }

    async fn run<F>(captured: Self::Captured, inner: F) -> Result<Response>
    where
        F: Future<Output = Result<Response>> + Send,
    {
        // Install the ability as ambient state for the handler (and the services
        // it calls), so the data layer can scope queries; then mask what comes
        // back. A missing ability means the `Authorize` extractor already rejected
        // the request with a 500, so this branch only carries it through.
        match captured {
            Some(ability) => {
                let resp = with_ability(ability.clone(), inner).await?;
                Ok(mask_response::<S>(&ability, A::ACTION, resp).await)
            }
            None => inner.await,
        }
    }
}

/// Mask a successful JSON body: deserialize it into `S::Model`(s), run the typed
/// masking, and re-serialize. A non-success or non-JSON response, or a scalar
/// body, passes through; a JSON object/array that does not match `S::Model`
/// fails closed (see module docs).
async fn mask_response<S>(ability: &Ability, action: Action, mut resp: Response) -> Response
where
    S: EntityTrait,
    S::Model: DeserializeOwned + Serialize,
{
    if !resp.status().is_success() {
        return resp;
    }
    let is_json = resp
        .content_type()
        .is_some_and(|ct| ct.starts_with("application/json"));
    if !is_json {
        return resp;
    }

    let bytes = match resp.take_body().into_bytes().await {
        Ok(bytes) => bytes,
        Err(_) => return resp,
    };

    // Deserialize straight into the typed model(s) — array vs object by the first
    // non-whitespace byte — skipping a `Value` round-trip. `mask_many` also drops
    // rows the actor may not see; a single object is field-masked only (the
    // handler owns the instance-level allow/deny for a by-id route).
    let masked = match bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) {
        Some(b'[') => serde_json::from_slice::<Vec<S::Model>>(bytes.as_ref())
            .map(|models| Value::Array(ability.mask_many::<S>(action, models.iter()))),
        Some(b'{') => serde_json::from_slice::<S::Model>(bytes.as_ref())
            .map(|model| ability.mask::<S>(action, &model)),
        // Not a maskable object/array (scalar, null, empty) — nothing to strip.
        _ => {
            resp.set_body(bytes);
            return resp;
        }
    };

    match masked.and_then(|value| serde_json::to_vec(&value)) {
        Ok(out) => {
            resp.set_body(out);
            resp
        }
        Err(_) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body("response masking failed: body did not match the authorized subject type"),
    }
}
