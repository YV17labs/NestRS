//! Authorization subjects: an entity, or a class no table holds.

use std::marker::PhantomData;

/// What a rule names and a posture gates: a SeaORM entity, whose rows a rule
/// filters and whose replies a posture masks, or a subject `#[subject]`
/// declares — a capability no table holds, which a rule grants whole and a
/// posture gates without masking.
///
/// ```
/// use std::any::TypeId;
///
/// use nest_rs_authz::{AbilityBuilder, Action, subject};
///
/// /// May export the org's reports.
/// #[subject]
/// pub struct Exporting;
///
/// let mut ab = AbilityBuilder::new();
/// ab.can(Action::Manage, Exporting).requires_scope("reports:export");
/// let ability = ab.build()?;
///
/// assert!(ability.can_class(Action::Manage, TypeId::of::<Exporting>()));
/// # Ok::<(), nest_rs_authz::MalformedRuleError>(())
/// ```
pub trait Subject: 'static {}

impl<E: sea_orm::EntityTrait> Subject for E {}

/// How a posture on a subject masks the reply of the operation it gates:
/// against an entity's rows, or not at all for a subject no table holds.
/// Implemented for every entity here and by `#[subject]`, never by hand.
pub trait SubjectMask: Subject {
    /// [`Rows<E>`] for an entity, [`NoRows`] for a `#[subject]`.
    type Mask;
}

impl<E: sea_orm::EntityTrait> SubjectMask for E {
    type Mask = Rows<E>;
}

/// The mask of an entity subject: a reply is reconciled with `E`'s rows.
pub struct Rows<E>(PhantomData<fn() -> E>);

/// The mask of a subject no table holds: a reply carries no row to mask.
pub struct NoRows;
