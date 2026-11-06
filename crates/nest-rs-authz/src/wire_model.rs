//! The round trip a reply mask runs on an `#[expose]`d entity's rows: a row
//! read back from what the wire carries, and written as the wire carries it.

use sea_orm::EntityTrait;
use serde::de::{DeserializeOwned, Error as _};
use serde_json::{Map, Value};

use crate::ability::FieldSet;

/// How a reply mask reads an `#[expose]`d entity's row off the wire and writes
/// it back, emitted by `#[expose]` and never written by hand.
///
/// Each column goes through its own serde impls, so the `Model` itself
/// serializes nowhere: a row crosses a wire only as its exposed type.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an `#[expose]`d entity, so no reply mask reads its rows",
    label = "no `#[expose]` on this entity's `Model`",
    note = "a mask rebuilds each row of a reply from the columns `#[expose]` declares: decorate the entity's `Model` with `#[expose(name = \"…\")]`"
)]
pub trait WireModel: EntityTrait {
    /// The key of every exposed column, as the entity's wire type writes it.
    const WIRE_KEYS: &'static [&'static str];

    /// The row's exposed columns, keyed as [`WIRE_KEYS`](Self::WIRE_KEYS)
    /// names them.
    ///
    /// # Errors
    ///
    /// A column whose value does not serialize.
    fn to_wire(model: &Self::Model) -> Result<Map<String, Value>, serde_json::Error>;

    /// The row a wire object describes: each exposed column read back, every
    /// other one given the placeholder `#[expose]` checked at compile time.
    ///
    /// # Errors
    ///
    /// An exposed column the object leaves out or carries in another shape.
    fn from_wire(wire: &Map<String, Value>) -> Result<Self::Model, serde_json::Error>;

    /// Whether [`from_wire`](Self::from_wire) invents this column's value, so a
    /// rule reading it is never decided on a rebuilt row.
    fn is_placeholdered(column: Self::Column) -> bool;

    /// The key this column crosses the wire under, `None` for one the wire
    /// never carries: an object that leaves the key out did not send the
    /// column, so a rule reading it is not decided on the value the rebuild
    /// gave it.
    fn wire_key(column: Self::Column) -> Option<&'static str>;

    /// The name [`wire_key`](Self::wire_key)'s column goes by in the database,
    /// `Column::as_str()`, for the column the wire carries under `key`: what a
    /// field grant names.
    fn key_column(key: &str) -> Option<&'static str>;
}

/// Whether a key ships: an exposed column the field grant permits, which it names
/// as the database does, whatever key the wire carries that column under.
pub(crate) fn kept_key<S: WireModel>(fields: &FieldSet, key: &str) -> bool {
    S::WIRE_KEYS.contains(&key)
        && match fields {
            FieldSet::All => true,
            FieldSet::Only(granted) => S::key_column(key).is_some_and(|c| granted.contains(c)),
        }
}

/// The exposed column `key` of a wire object, read through its own
/// `Deserialize`: a key the object leaves out reads as `null`, which only an
/// `Option` column takes, as serde's derive reads a missing field.
///
/// # Errors
///
/// The key absent from a column no `null` reads, or a value of another shape.
pub fn wire_column<T: DeserializeOwned>(
    wire: &Map<String, Value>,
    key: &'static str,
) -> Result<T, serde_json::Error> {
    match wire.get(key) {
        Some(value) => T::deserialize(value),
        None => T::deserialize(&Value::Null)
            .map_err(|_refused_null| serde_json::Error::missing_field(key)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_present_column_reads_through_its_own_type() {
        let mut wire = Map::new();
        wire.insert("count".into(), serde_json::json!(7));
        assert_eq!(wire_column::<i64>(&wire, "count").ok(), Some(7));
        assert_eq!(
            wire_column::<Option<i64>>(&wire, "count").ok(),
            Some(Some(7))
        );
    }

    #[test]
    fn a_missing_column_reads_as_none_only_where_it_is_optional() {
        let wire = Map::new();
        assert_eq!(
            wire_column::<Option<String>>(&wire, "note").ok(),
            Some(None)
        );
        let refused = wire_column::<String>(&wire, "note")
            .err()
            .map(|err| err.to_string());
        assert_eq!(refused.as_deref(), Some("missing field `note`"));
    }

    /// serde's own sentence quotes the value; the message every mask failure
    /// logs, `error_message`'s, says the kind alone.
    #[test]
    fn a_value_of_another_shape_is_refused_without_quoting_it() {
        let mut wire = Map::new();
        wire.insert("count".into(), serde_json::json!("seven"));
        let refused = wire_column::<i64>(&wire, "count").expect_err("a string is no `i64`");
        let logged = nest_rs_core::error_message(&refused);
        assert!(
            logged.contains("expected i64") && !logged.contains("seven"),
            "{logged}",
        );
    }
}
