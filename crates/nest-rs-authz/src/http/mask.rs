//! `Authorize<A, S>` as a [`RouteResponseShaper`] declares the route's class
//! gate, run with its guards, and — on an entity subject — arms the route's
//! response mask, keyed on the ability the guard attached; a subject no table
//! holds arms none, so its route reads as unmasked:
//!
//! 1. A masked route is one whose reads run under that ability: the data layer
//!    scopes every query via `current_ability()` — no hand-written filter.
//! 2. Masks the JSON response: read each row back into `S::Model` through what
//!    `#[expose]` declares (an unexposed column takes the placeholder it
//!    checked at compile time), judge it as [`Ability::mask_many`] does, then
//!    retain only the entity's exposed columns its verdict grants, so neither
//!    an unrestricted field grant nor a body carrying a hidden column can leak
//!    one (e.g. `password_hash`, which carries no `#[expose]`).
//!
//! Fails **closed**: a successful JSON body that cannot be reconciled with
//! `S::Model` yields 500 rather than shipping data unmasked.

use crate::ability::{Provenance, mask_reason};
use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_core::{Edge, ServedAddress};
use nest_rs_http::__private::ServedRoute;
use nest_rs_http::{
    Request, Response, ResponseShaping, RouteFuture, RouteResponseShaper, StatusCode, header,
};
use sea_orm::EntityTrait;
use serde_json::Value;

use super::authorize::Authorize;
use crate::subject::{NoRows, Rows, SubjectMask};
use crate::wire_mask::{MaskedUnit, MaskedWire, mask_wire_json, warn_mask_failure};
use crate::wire_model::WireModel;
use crate::{Ability, Action, ActionMarker};

impl<A, S> RouteResponseShaper for Authorize<A, S>
where
    A: ActionMarker,
    S: SubjectMask,
    S::Mask: RouteMask,
{
    const GATE: Option<nest_rs_http::__private::GateFn> = Some(crate::class_gate::<A, S>);
    const SHAPES: bool = <S::Mask as RouteMask>::SHAPES;

    fn capture(req: &Request) -> Option<Box<dyn ResponseShaping>> {
        // A missing ability means the class gate already answered 500, so there
        // is nothing to shape.
        let ability = crate::context::installed()?;
        <S::Mask as RouteMask>::shaping(ability, A::ACTION, ServedRoute::of(req))
    }
}

/// How a route's posture shapes the reply on a subject's [`SubjectMask::Mask`]:
/// an entity's rows mask it, a subject no table holds leaves it unshaped.
#[diagnostic::on_unimplemented(
    message = "this posture arms no reply mask for this subject",
    label = "neither an `#[expose]`d entity nor a `#[subject]`",
    note = "a posture masks its reply against its entity's `#[expose]`d wire model, read and written as the entity's rows: expose the entity and answer a value that serializes, or declare a capability no table holds with `#[subject]`, whose posture masks nothing"
)]
pub trait RouteMask {
    /// Whether the posture shapes the reply at all.
    const SHAPES: bool;

    /// The shaping step for `action` on the request `route` names, keyed on the
    /// ability the guard attached.
    fn shaping(
        ability: Arc<Ability>,
        action: Action,
        route: ServedRoute,
    ) -> Option<Box<dyn ResponseShaping>>;
}

#[diagnostic::do_not_recommend]
impl<E> RouteMask for Rows<E>
where
    E: WireModel,
{
    const SHAPES: bool = true;

    fn shaping(
        ability: Arc<Ability>,
        action: Action,
        route: ServedRoute,
    ) -> Option<Box<dyn ResponseShaping>> {
        Some(Box::new(AbilityShaping::<E>::new(ability, action, route)))
    }
}

impl RouteMask for NoRows {
    const SHAPES: bool = false;

    fn shaping(_: Arc<Ability>, _: Action, _: ServedRoute) -> Option<Box<dyn ResponseShaping>> {
        None
    }
}

/// The caller's ability plus the entity's mask, captured before the handler and
/// ready to mask what it answers.
struct AbilityShaping<S> {
    ability: Arc<Ability>,
    action: Action,
    route: ServedRoute,
    subject: PhantomData<fn() -> S>,
}

impl<S> AbilityShaping<S> {
    /// The shaping step for `action` on entity `S`, keyed on the ability the
    /// guard attached, a failure naming the request `route` names.
    fn new(ability: Arc<Ability>, action: Action, route: ServedRoute) -> Self {
        Self {
            ability,
            action,
            route,
            subject: PhantomData,
        }
    }
}

impl<S> ResponseShaping for AbilityShaping<S>
where
    S: WireModel,
{
    fn apply<'a>(self: Box<Self>, inner: RouteFuture<'a>) -> RouteFuture<'a> {
        Box::pin(async move {
            let resp = inner.await?;
            let unit = MaskedUnit::on(Edge::Http, self.route.handler(), self.route.address());
            Ok(masked_response::<S>(unit, &self.ability, self.action, resp).await)
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
/// JSON per RFC 9110 §8.3.1 (type and subtype case-insensitive) and RFC 6839
/// (a `+json` suffix), parameters ignored. Undeclared is its own answer: passing
/// such a body through would be the shaper guessing, toward the leak.
fn body_kind(resp: &Response) -> BodyKind {
    let Some(raw) = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
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
/// Refused unless empty: an armed route's body carries the entity. Emptiness is
/// read off the size hint, so a streamed body is refused without being buffered.
fn refuse_unclassifiable<S: EntityTrait>(
    unit: MaskedUnit<'_>,
    mut resp: Response,
    action: Action,
) -> Response {
    let body = resp.take_body();
    if body.is_empty() {
        resp.set_body(body);
        return resp;
    }
    mask_failure::<S>(
        unit,
        action,
        mask_reason::UNCLASSIFIED_BODY,
        "response carried no content type, so it could not be classified; set a content type on \
         the response, or return a typed body",
        None,
        "response masking failed: response carried no content type",
    )
}

/// Mask a successful JSON body of an `#[expose]`d entity: read each row back
/// into `S::Model`, judge it, and serialize what its verdict grants. A
/// non-success response, or one whose handler declared a non-JSON media type,
/// passes through; a JSON object/array that does not match `S::Model`, and a
/// body with no declared type at all, fail closed (see module docs).
pub async fn mask_entity_response<S>(ability: &Ability, action: Action, resp: Response) -> Response
where
    S: WireModel,
{
    let unit = MaskedUnit::on(Edge::Http, None, ServedAddress::default());
    masked_response::<S>(unit, ability, action, resp).await
}

/// [`mask_entity_response`], what it refuses naming `unit`.
async fn masked_response<S>(
    unit: MaskedUnit<'_>,
    ability: &Ability,
    action: Action,
    mut resp: Response,
) -> Response
where
    S: WireModel,
{
    if !resp.status().is_success() {
        return resp;
    }
    match body_kind(&resp) {
        BodyKind::Json => {}
        // The handler declared a type, and it is not JSON: a file, a redirect
        // body, a rendered page. There is no entity in it to mask.
        BodyKind::Other => return resp,
        BodyKind::Undeclared => return refuse_unclassifiable::<S>(unit, resp, action),
    }

    let bytes = match resp.take_body().into_bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            // The body is already consumed; there is nothing left to ship, so
            // fail closed rather than return an empty 200.
            return mask_failure::<S>(
                unit,
                action,
                mask_reason::NOT_SERIALIZABLE,
                "response body could not be read",
                Some(&err),
                "response masking failed: could not read response body",
            );
        }
    };

    let wire: Value = match serde_json::from_slice(bytes.as_ref()) {
        Ok(wire) => wire,
        Err(err) => {
            return mask_failure::<S>(
                unit,
                action,
                mask_reason::NOT_SERIALIZABLE,
                "response body was not valid JSON",
                Some(&err),
                "response masking failed: body was not valid JSON",
            );
        }
    };

    match mask_wire_json::<S>(ability, action, wire, Provenance::Loaded) {
        Ok(MaskedWire::Passthrough(_)) => {
            resp.set_body(bytes);
            resp
        }
        Ok(MaskedWire::Masked(value)) => match serde_json::to_vec(&value) {
            Ok(out) => {
                resp.set_body(out);
                resp
            }
            Err(err) => mask_failure::<S>(
                unit,
                action,
                mask_reason::NOT_SERIALIZABLE,
                "masked body did not serialize",
                Some(&err),
                MASKING_FAILED_BODY,
            ),
        },
        Err(err) => mask_failure::<S>(
            unit,
            action,
            mask_reason::IRRECONCILABLE,
            "response body did not match the authorized subject type",
            Some(&err),
            MASKING_FAILED_BODY,
        ),
    }
}

/// The body a fail-closed masking exit ships when the value could not be
/// reconciled with the authorized subject type.
const MASKING_FAILED_BODY: &str =
    "response masking failed: body did not match the authorized subject type";

/// One shape for every fail-closed masking exit: the queryable `warn` plus the
/// 500 that ships instead of unmasked data.
fn mask_failure<S: EntityTrait>(
    unit: MaskedUnit<'_>,
    action: Action,
    reason: &'static str,
    detail: &'static str,
    err: Option<&(dyn std::error::Error + 'static)>,
    body: &'static str,
) -> Response {
    warn_mask_failure(S::default().table_name(), action, reason, detail, unit, err);
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(body)
}
