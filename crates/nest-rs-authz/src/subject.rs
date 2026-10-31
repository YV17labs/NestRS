//! Authorization subjects: an entity, or a class no table holds.

use std::marker::PhantomData;

use sea_orm::{EntityTrait, IdenStatic};

/// What a rule names and a posture gates: a SeaORM entity, whose rows a rule
/// filters and whose replies a posture masks, or a subject `#[subject]`
/// declares — a capability no table holds, which a rule grants whole and a
/// posture gates without masking.
///
/// ```
/// use std::any::TypeId;
///
/// use nest_rs_authz::{AbilityBuilder, Action, subject};
/// use nest_rs_guards::Scope;
///
/// /// May export the org's reports.
/// #[subject]
/// pub struct Exporting;
///
/// const REPORTS_EXPORT: Scope = Scope::new("reports:export");
///
/// let mut ab = AbilityBuilder::new();
/// ab.can(Action::Manage, Exporting).requires_scope(REPORTS_EXPORT);
/// let ability = ab.build()?;
///
/// assert!(ability.can_class(Action::Manage, TypeId::of::<Exporting>()));
/// # Ok::<(), nest_rs_authz::MalformedRuleError>(())
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an authorization subject",
    label = "neither an entity nor a `#[subject]`",
    note = "a posture names a SeaORM entity, whose rows it masks, or a capability no table holds, declared with `#[subject]` on a unit struct: `#[subject] pub struct Exporting;`"
)]
pub trait Subject: 'static {
    /// Whether the subject has rows a posture masks: `false` for a
    /// `#[subject]`, so an extractor naming it owes the masking probe nothing.
    #[doc(hidden)]
    const __ROWS: bool = true;
}

#[diagnostic::do_not_recommend]
impl<E: sea_orm::EntityTrait> Subject for E {}

/// A subject whose rows a rule narrows: a SeaORM entity, which `cannot`,
/// `when` and `fields` take, and never a capability `#[subject]` declares.
///
/// ```compile_fail,E0277
/// use nest_rs_authz::{AbilityBuilder, Action, subject};
///
/// #[subject]
/// pub struct Exporting;
///
/// let mut ab = AbilityBuilder::new();
/// ab.cannot(Action::Manage, Exporting);
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no rows to deny or narrow: `cannot`, `when` and `fields` take a SeaORM entity",
    label = "not a SeaORM entity",
    note = "a capability `#[subject]` declares has no rows: a grant covers it whole, and a grant not written is its denial"
)]
pub trait RowSubject: Subject + EntityTrait<Column: Send + Sync + 'static> {
    /// The entity's `Column`, named through this trait so a capability's
    /// `fields` is refused by its sentence rather than by `EntityTrait`.
    #[doc(hidden)]
    type __Column: IdenStatic;
}

#[diagnostic::do_not_recommend]
impl<E> RowSubject for E
where
    E: EntityTrait<Column: Send + Sync + 'static>,
{
    type __Column = E::Column;
}

/// How a posture on a subject masks the reply of the operation it gates:
/// against an entity's rows, or not at all for a subject no table holds.
/// Implemented for every entity here and by `#[subject]`, never by hand.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an authorization subject",
    label = "neither an entity nor a `#[subject]`",
    note = "a posture names a SeaORM entity, whose rows it masks, or a capability no table holds, declared with `#[subject]` on a unit struct: `#[subject] pub struct Exporting;`"
)]
pub trait SubjectMask: Subject {
    /// [`Rows<E>`] for an entity, [`NoRows`] for a `#[subject]`.
    type Mask;
}

#[diagnostic::do_not_recommend]
impl<E: sea_orm::EntityTrait> SubjectMask for E {
    type Mask = Rows<E>;
}

/// The mask of an entity subject: a reply is reconciled with `E`'s rows.
pub struct Rows<E>(PhantomData<fn() -> E>);

/// The mask of a subject no table holds: a reply carries no row to mask.
pub struct NoRows;
