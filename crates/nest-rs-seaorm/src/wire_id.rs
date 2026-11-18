use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use uuid::Uuid;

/// An id on an `#[expose]` wire row or in its `Create`/`Update` input, whether
/// its column is an `Option` or not: the UUID's hyphenated text on every edge,
/// and on GraphQL the `ID` a `#[crud]` id argument takes. It is as a `Uuid` is —
/// copied, compared, hashed and printed alike — reads back as the column's `Uuid`
/// through `AsRef` and `Into`, and is built from one with `into()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct WireId(Uuid);

impl fmt::Display for WireId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<Uuid> for WireId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

impl From<&Uuid> for WireId {
    fn from(id: &Uuid) -> Self {
        Self(*id)
    }
}

impl From<WireId> for Uuid {
    fn from(id: WireId) -> Self {
        id.0
    }
}

impl AsRef<Uuid> for WireId {
    fn as_ref(&self) -> &Uuid {
        &self.0
    }
}

// By hand, as `WireDateTime`'s: the derive's `transparent` gives way to a
// documented type.
impl JsonSchema for WireId {
    fn inline_schema() -> bool {
        <Uuid as JsonSchema>::inline_schema()
    }

    fn schema_name() -> Cow<'static, str> {
        <Uuid as JsonSchema>::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        <Uuid as JsonSchema>::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        <Uuid as JsonSchema>::json_schema(generator)
    }
}

#[cfg(feature = "graphql")]
mod graphql {
    use async_graphql::{InputValueError, InputValueResult, Scalar, ScalarType, Value};
    use uuid::Uuid;

    use super::WireId;

    // Registered under async-graphql's own `ID`, which a schema prints once: it
    // keeps that scalar's description (none) and the literals it accepts.
    #[Scalar(name = "ID")]
    impl ScalarType for WireId {
        #[expect(
            clippy::map_err_ignore,
            reason = "the refusal names the expected shape, never the parser's internals"
        )]
        fn parse(value: Value) -> InputValueResult<Self> {
            let refused = || InputValueError::custom("expected a UUID");
            match &value {
                Value::String(written) => Uuid::parse_str(written).map(Self).map_err(|_| refused()),
                _ => Err(refused()),
            }
        }

        fn is_valid(value: &Value) -> bool {
            match value {
                Value::Number(n) => n.is_i64(),
                Value::String(_) => true,
                _ => false,
            }
        }

        fn to_value(&self) -> Value {
            Value::String(self.0.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V7: &str = "01920000-0000-7000-8000-0000000001a1";
    const V4: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";

    fn id(text: &str) -> WireId {
        WireId(Uuid::parse_str(text).expect("a UUID"))
    }

    #[test]
    fn an_id_serializes_and_reads_back_as_the_uuid_s_text() {
        let id = id(V7);
        assert_eq!(serde_json::to_value(id).expect("serializes"), V7);
        let read: WireId = serde_json::from_value(V7.into()).expect("reads back");
        assert_eq!(Uuid::from(read), *id.as_ref());
    }

    #[test]
    fn the_schema_is_the_uuid_s() {
        assert_eq!(schemars::schema_for!(WireId), schemars::schema_for!(Uuid),);
    }

    /// Any version reads back, a key another system drew included.
    #[test]
    fn an_id_of_any_uuid_version_reads_back() {
        for text in [V7, V4] {
            let read: WireId = serde_json::from_value(text.into()).expect("a UUID reads back");
            assert_eq!(read.to_string(), text);
        }
    }

    /// Borrowed where a field resolver holds its parent, an id is copied out,
    /// compared, hashed and printed as the `Uuid` it holds.
    #[test]
    fn an_id_is_as_its_uuid_is() {
        let parent = (id(V7), id(V7), id(V4));
        let (held, same, other) = (&parent.0, &parent.1, &parent.2);
        assert_eq!(Uuid::from(*held), Uuid::parse_str(V7).expect("a UUID"));
        assert!(held == same && held != other);
        assert_eq!(held.to_string(), Uuid::from(*held).to_string());
        let set: std::collections::HashSet<WireId> = [*held, *same, *other].into();
        assert_eq!(set.len(), 2);
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn graphql_writes_and_reads_the_text_every_other_edge_writes() {
        use async_graphql::{ScalarType, Value};

        let id = id(V7);
        assert_eq!(ScalarType::to_value(&id), Value::String(V7.to_owned()));
        let read = <WireId as ScalarType>::parse(Value::String(V7.to_owned()))
            .expect("its own text parses");
        assert_eq!(Uuid::from(read), *id.as_ref());
        assert!(<WireId as ScalarType>::parse(Value::String("not-a-uuid".to_owned())).is_err());
    }
}
