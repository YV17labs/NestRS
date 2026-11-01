//! Field-level response masking for MCP operations — the transport analog of
//! [`crate::graphql::masked_value_for`].
//!
//! The framework-carried path is [`masked_value_for`], emitted automatically by
//! `#[tools]` after every `#[authorize(Action, Entity)]`-declared operation; a
//! host never calls it. [`crate::masked_output_ambient`] remains the manual
//! primitive for a hand-written `ServerHandler` surface, which no decorator
//! reaches.

use nest_rs_core::Edge;
use nest_rs_guards::Denial;
use nest_rs_mcp::McpError;
use nest_rs_mcp::denial_to_mcp_error;
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ability::{Provenance, mask_reason};
use crate::gate::{Refusal, reason};
use crate::subject::{NoRows, Rows, SubjectMask};
use crate::wire_mask::{MaskedWire, mask_wire_detail, mask_wire_json, warn_mask_failure};
use crate::wire_model::WireModel;
use crate::{Ability, Action, ActionMarker, current_ability};

/// What the developer changes so this refusal has a representation; rides in `remedy`.
const NULLABLE_REMEDY: &str = "make the column `Option` on the entity, so a field grant's refusal has a \
     representation in the operation's return type";

/// Mask an operation's already-built value through the ambient ability, sharing
/// the value-level round-trip every transport masks with
/// (`crate::wire_mask`): serialize, read each object back into `E::Model`
/// through what `#[expose]` declares, judge it as
/// [`Ability::mask_many`](crate::Ability::mask_many) does, retain the exposed
/// wire keys its verdict grants, deserialize back. Rows the ability refuses are
/// dropped; scalars and `null` pass through untouched.
///
/// MCP has no selection set: a mask that takes a key the return type requires
/// **refuses the operation**. A column an ability rule may mask should be
/// `Option` on the entity.
pub fn masked_value_for<A, E, O>(value: O) -> Result<O, McpError>
where
    A: ActionMarker,
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let action = A::ACTION;
    let Some(ability) = current_ability() else {
        // Fail closed: nothing decided what this caller may read.
        return Err(mask_failure::<E>(
            action,
            mask_reason::NO_AMBIENT_ABILITY,
            "no ambient ability — does an ability guard run at the /mcp request?",
            None,
        ));
    };
    let wire = match serde_json::to_value(&value) {
        Ok(wire) => wire,
        Err(err) => {
            return Err(mask_failure::<E>(
                action,
                mask_reason::NOT_SERIALIZABLE,
                "operation value did not serialize",
                Some(&err),
            ));
        }
    };
    match mask_wire_json::<E>(&ability, action, wire, Provenance::Loaded) {
        Ok(MaskedWire::Passthrough(_)) => Ok(value),
        Ok(MaskedWire::Masked(masked)) => match serde_json::from_value(masked) {
            Ok(masked) => Ok(masked),
            // The mask took a key the return type requires. Unlike GraphQL there
            // is no selection set to acquit it with, so this is the refusal.
            Err(_) => Err(refused_fields::<E, O>(&ability, action, &value)),
        },
        Err(err) => Err(mask_failure::<E>(
            action,
            mask_reason::IRRECONCILABLE,
            "value could not be reconciled with the subject model",
            Some(&err),
        )),
    }
}

/// The refusal a field grant makes on a shape that cannot express it.
///
/// Filed as a denial: the same event and `reason` GraphQL files for a selected
/// stripped key.
fn refused_fields<E, O>(ability: &Ability, action: Action, value: &O) -> McpError
where
    E: WireModel,
    O: Serialize,
{
    // Written again rather than kept: the mask consumed the first, and only a
    // refusal comes this way.
    let wire = match serde_json::to_value(value) {
        Ok(wire) => wire,
        Err(err) => {
            return mask_failure::<E>(
                action,
                mask_reason::NOT_SERIALIZABLE,
                "operation value did not serialize",
                Some(&err),
            );
        }
    };
    // Counted before the mask consumes the value, to tell two refusals apart
    // below.
    let declared = wire.as_object().map_or(0, serde_json::Map::len);
    let removed = match mask_wire_detail::<E>(ability, action, wire) {
        Ok(detail) => detail.removed,
        // Unreachable short of the value's and the entity's serde disagreeing;
        // reported as the masking failure it then is.
        Err(err) => {
            return mask_failure::<E>(
                action,
                mask_reason::IRRECONCILABLE,
                "value could not be reconciled with the subject model",
                Some(&err),
            );
        }
    };
    // Every field removed is the class refusing the row (`Ability::mask` does not
    // consult `can`), not a field grant: no `fields`, no nullable remedy.
    let whole_subject = declared > 0 && removed.len() == declared;
    let fields = removed.into_iter().collect::<Vec<_>>().join(",");
    crate::gate::warn_denied(Refusal {
        subject: Some(std::any::type_name::<E>()),
        action: Some(action),
        // Joined, a tracing field being a scalar; omitted when it is every key.
        fields: (!whole_subject).then_some(&fields),
        reason: Some(if whole_subject {
            reason::NO_CLASS_GRANT
        } else {
            reason::FIELD_NOT_GRANTED
        }),
        remedy: (!whole_subject).then_some(NULLABLE_REMEDY),
        ..Refusal::on(Edge::Mcp.as_str())
    });
    // The gate's own vocabulary: a mask refusal tells no more than a gate's.
    denial_to_mcp_error(Denial::forbidden("forbidden"))
}

/// How an operation's posture masks its value on a subject's
/// [`SubjectMask::Mask`]: an entity's rows mask it, a subject no table holds
/// lets it through untouched.
#[diagnostic::on_unimplemented(
    message = "`#[authorize]` arms no reply mask for this subject",
    label = "neither an `#[expose]`d entity nor a `#[subject]`",
    note = "a posture masks its reply against its entity's `#[expose]`d wire model, read and written as the entity's rows: expose the entity and answer a value that serializes, or declare a capability no table holds with `#[subject]`, whose posture masks nothing"
)]
pub trait McpMask<A, O> {
    /// [`masked_value_for`]'s answer on an entity; the value itself otherwise.
    fn masked_value(value: O) -> Result<O, McpError>;
}

#[diagnostic::do_not_recommend]
impl<A, E, O> McpMask<A, O> for Rows<E>
where
    A: ActionMarker,
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    fn masked_value(value: O) -> Result<O, McpError> {
        masked_value_for::<A, E, O>(value)
    }
}

impl<A, O> McpMask<A, O> for NoRows {
    fn masked_value(value: O) -> Result<O, McpError> {
        Ok(value)
    }
}

/// The value an `#[authorize(A, S)]` operation answers, masked as `S` masks it.
pub fn mcp_masked_value<A, S, O>(value: O) -> Result<O, McpError>
where
    S: SubjectMask,
    S::Mask: McpMask<A, O>,
{
    <S::Mask as McpMask<A, O>>::masked_value(value)
}

/// One shape for every fail-closed masking exit: the queryable `warn` plus an
/// error that names neither the column nor the reason — the reader is a model.
fn mask_failure<E>(
    action: Action,
    reason: &'static str,
    detail: &'static str,
    err: Option<&serde_json::Error>,
) -> McpError
where
    E: EntityTrait,
{
    warn_mask_failure(
        std::any::type_name::<E>(),
        action,
        reason,
        detail,
        Some(Edge::Mcp.as_str()),
        None,
        err.map(|e| e as &(dyn std::error::Error + 'static)),
    );
    denial_to_mcp_error(Denial::internal("response masking failed"))
}
