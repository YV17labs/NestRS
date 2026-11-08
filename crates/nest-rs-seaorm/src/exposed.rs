//! [`Exposed`] — the wire type an entity's `#[expose]` generates.

use sea_orm::EntityTrait;

/// An entity `#[expose]` declares, and the wire type it answers with: the
/// struct named by `#[expose(name = "…")]`, built from a row, which `#[crud]`
/// returns on every edge.
///
/// Sealed: only the decorator implements it, since a hand-written impl would
/// put on the wire a type no `#[expose]` built.
///
/// ```
/// # use nest_rs_seaorm::{Exposed, expose};
/// # mod user {
/// #     use super::*;
/// #     use sea_orm::entity::prelude::*;
/// #[expose(name = "User")]
/// #[sea_orm::model]
/// # #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// # #[sea_orm(table_name = "users")]
/// pub struct Model {
///     #[sea_orm(primary_key, auto_increment = false)]
///     #[expose]
///     pub id: Uuid,
///     #[expose]
///     pub name: String,
/// }
/// # impl ActiveModelBehavior for ActiveModel {}
/// # }
/// fn wire_of(row: &user::Model) -> <user::Entity as Exposed>::Wire {
///     From::from(row)
/// }
/// # fn main() {
///
/// let row = user::Model { id: Default::default(), name: "Ada".into() };
/// let wire: user::User = wire_of(&row);
/// assert_eq!(wire.name, "Ada");
/// # }
/// ```
///
/// A hand-written impl is refused:
///
/// ```compile_fail,E0277
/// use nest_rs_seaorm::Exposed;
/// use nest_rs_seaorm::sea_orm;
/// use sea_orm::entity::prelude::*;
///
/// #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// #[sea_orm(table_name = "notes")]
/// pub struct Model {
///     #[sea_orm(primary_key)]
///     pub id: i32,
/// }
///
/// #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// pub enum Relation {}
///
/// impl ActiveModelBehavior for ActiveModel {}
///
/// pub struct Note;
///
/// impl From<&Model> for Note {
///     fn from(_: &Model) -> Self {
///         Note
///     }
/// }
///
/// impl Exposed for Entity {
///     type Wire = Note;
/// }
/// ```
pub trait Exposed: EntityTrait + ExposeDeclared {
    /// The wire type `#[expose(name = "…")]` generates, built from a row.
    type Wire: for<'a> From<&'a Self::Model> + Send + Sync + 'static;
}

/// The seal of [`Exposed`]: implemented by the `#[expose]` expansion beside
/// the wire type it generates, never by hand.
pub trait ExposeDeclared {}

/// The schema of a wire field `#[expose(nullable)]` declares: its value's, or
/// `null`, in `format` where its column's type keeps one. The field stays required,
/// since a reply always writes its key.
pub fn nullable_schema<T: schemars::JsonSchema>(
    generator: &mut schemars::SchemaGenerator,
    format: Option<&'static str>,
) -> schemars::Schema {
    let mut schema = generator.subschema_for::<Option<T>>();
    if let Some(format) = format {
        schema.insert("format".to_owned(), format.into());
    }
    schema
}
