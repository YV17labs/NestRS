//! `Authorize<A, S>` as a [`RouteResponseShaper`] does two things keyed on the
//! ability the guard attached:
//!
//! 1. Wraps the handler in [`with_ability`] so the data layer scopes every
//!    query via `current_ability()` — no hand-written filter.
//! 2. Masks the JSON response: parse into `S::Model` (filling the unexposed
//!    columns the `#[expose]` output omits via [`WireModelDefaults`]), run typed
//!    [`Ability::mask`] / [`Ability::mask_many`], then retain only the entity's
//!    statically-known exposed columns ([`WireModelDefaults::wire_keys`]) so
//!    neither an unrestricted field grant nor a handler returning a raw `Model`
//!    can leak an unexposed column (e.g. `password_hash`, which carries no
//!    `#[expose]`). Keying on the static set rather than the response body keeps
//!    this sound even when `mask_many` drops rows.
//!
//! Fails **closed**: a successful JSON body that cannot be reconciled with
//! `S::Model` yields 500 rather than shipping data unmasked.

use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_http::{ResponseShaping, RouteFuture, RouteResponseShaper};
use nest_rs_resource::WireModelDefaults;
use poem::http::StatusCode;
use poem::{Request, Response};
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::authorize::Authorize;
use crate::wire_mask::{MaskedWire, mask_wire_json, warn_mask_failure};
use crate::{Ability, Action, ActionMarker, with_ability};

impl<A, S> RouteResponseShaper for Authorize<A, S>
where
    A: ActionMarker,
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    fn capture(req: &Request) -> Option<Box<dyn ResponseShaping>> {
        // A missing ability means the extractor already rejected with 500, so
        // there is nothing to shape.
        let ability = req.extensions().get::<Arc<Ability>>().cloned()?;
        Some(Box::new(AbilityShaping::<S>::new(ability, A::ACTION)))
    }
}

/// The ambient ability plus the entity's mask, captured off the request and
/// ready to wrap the handler. Shared by `Authorize<A, S>` and `nest-rs-seaorm`'s
/// `Bind<A, S>`, which differ only in how they name the entity.
pub struct AbilityShaping<S> {
    ability: Arc<Ability>,
    action: Action,
    subject: PhantomData<fn() -> S>,
}

impl<S> AbilityShaping<S> {
    /// The shaping step for `action` on entity `S`, keyed on the ability the
    /// guard attached.
    pub fn new(ability: Arc<Ability>, action: Action) -> Self {
        Self {
            ability,
            action,
            subject: PhantomData,
        }
    }
}

impl<S> ResponseShaping for AbilityShaping<S>
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    fn apply<'a>(self: Box<Self>, inner: RouteFuture<'a>) -> RouteFuture<'a> {
        Box::pin(async move {
            let resp = with_ability(self.ability.clone(), inner).await?;
            Ok(mask_entity_response::<S>(&self.ability, self.action, resp).await)
        })
    }
}

/// What a response's declared media type licenses the shaper to do.
enum BodyKind {
    /// A JSON media type — mask it.
    Json,
    /// Another media type the handler named. Nothing in it to mask.
    Other,
    /// No media type at all, so nothing says which of the two it is.
    Undeclared,
}

/// The response's media type read as a **type**, not as a spelling.
///
/// The arming is type-directed and cannot be renamed out of, but what it arms
/// used to decide whether to run by comparing the `Content-Type` against the
/// literal prefix `application/json`. Three responses that are JSON by the
/// standard failed that test and shipped every unexposed column:
///
/// - `Application/JSON` — RFC 9110 §8.3.1 makes type and subtype
///   case-insensitive, so this is the *same* media type;
/// - `application/vnd.api+json`, `application/problem+json` — RFC 6839 makes
///   `+json` a structured syntax suffix, so these are JSON too;
/// - a media type carrying parameters, which are not part of the type.
///
/// Undeclared is deliberately its own answer rather than folded into `Other`:
/// passing a body through because it *named* a non-JSON type is a decision the
/// handler made, while passing one through because it named nothing is the
/// shaper guessing — and guessing in the direction that leaks.
fn body_kind(resp: &Response) -> BodyKind {
    let Some(raw) = resp.content_type() else {
        return BodyKind::Undeclared;
    };
    let essence = raw.split(';').next().unwrap_or_default().trim();
    let Some((_type, subtype)) = essence.split_once('/') else {
        return BodyKind::Other;
    };
    let subtype = subtype.trim();
    let is_json = subtype.eq_ignore_ascii_case("json")
        || subtype
            .rsplit_once('+')
            .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("json"));
    if is_json {
        BodyKind::Json
    } else {
        BodyKind::Other
    }
}

/// A success with a body and no declared media type.
///
/// Nothing licenses a pass-through: an armed route is one whose posture says the
/// response carries this entity, and shipping a body the shaper cannot classify
/// is the silent direction of the only error that matters here. An *empty* body
/// is not that — there is nothing in it to leak — and emptiness is read off the
/// body's size hint rather than by collecting it, so a streamed download on a
/// route that forgot its `Content-Type` is refused without being buffered whole
/// first.
fn refuse_unclassifiable<S>(mut resp: Response, action: Action) -> Response {
    let body = resp.take_body();
    if body.is_empty() {
        resp.set_body(body);
        return resp;
    }
    warn_mask_failure(
        std::any::type_name::<S>(),
        action,
        "response carried no content type, so it could not be classified",
        &"set a content type on the response, or return a typed body",
    );
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body("response masking failed: response carried no content type")
}

/// Mask a successful JSON body: deserialize it into `S::Model`(s), run the typed
/// masking, and re-serialize. A non-success response, or one whose handler
/// declared a non-JSON media type, passes through; a JSON object/array that does
/// not match `S::Model`, and a body with no declared type at all, fail closed
/// (see module docs).
pub async fn mask_entity_response<S>(
    ability: &Ability,
    action: Action,
    mut resp: Response,
) -> Response
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    if !resp.status().is_success() {
        return resp;
    }
    match body_kind(&resp) {
        BodyKind::Json => {}
        // The handler declared a type, and it is not JSON: a file, a redirect
        // body, a rendered page. There is no entity in it to mask.
        BodyKind::Other => return resp,
        BodyKind::Undeclared => return refuse_unclassifiable::<S>(resp, action),
    }

    let bytes = match resp.take_body().into_bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            // The body is already consumed; there is nothing left to ship, so
            // fail closed rather than return an empty 200.
            warn_mask_failure(
                std::any::type_name::<S>(),
                action,
                "response body could not be read",
                &err,
            );
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("response masking failed: could not read response body");
        }
    };

    // Round-trip handler DTOs through `S::Model` for policy, then drop any
    // fields the wire shape never carried (e.g. `password_hash`).
    let wire: Value = match serde_json::from_slice(bytes.as_ref()) {
        Ok(wire) => wire,
        Err(err) => {
            warn_mask_failure(
                std::any::type_name::<S>(),
                action,
                "response body was not valid JSON",
                &err,
            );
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("response masking failed: body was not valid JSON");
        }
    };

    // One masking semantics for every transport: the value-level round-trip
    // (wire → `S::Model` → mask → exposed-key strainer) lives in
    // `crate::wire_mask`, shared with the GraphQL resolver wrapper.
    match mask_wire_json::<S>(ability, action, &wire) {
        Ok(MaskedWire::Passthrough) => {
            // Scalar / null — nothing to strip.
            resp.set_body(bytes);
            resp
        }
        Ok(MaskedWire::Masked(value)) => match serde_json::to_vec(&value) {
            Ok(out) => {
                resp.set_body(out);
                resp
            }
            Err(err) => {
                warn_mask_failure(
                    std::any::type_name::<S>(),
                    action,
                    "masked body did not serialize",
                    &err,
                );
                masking_failed()
            }
        },
        Err(err) => {
            warn_mask_failure(
                std::any::type_name::<S>(),
                action,
                "response body did not match the authorized subject type",
                &err,
            );
            masking_failed()
        }
    }
}

/// The fail-closed response: a successful body that cannot be reconciled with
/// the authorized subject type ships a 500, never unmasked data.
fn masking_failed() -> Response {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body("response masking failed: body did not match the authorized subject type")
}
