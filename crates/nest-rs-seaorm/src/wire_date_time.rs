use std::borrow::Cow;

use chrono::{DateTime, FixedOffset, TimeZone};
use schemars::{JsonSchema, Schema, SchemaGenerator};

/// An instant on an `#[expose]` wire row, whatever chrono type its column holds:
/// one RFC 3339 string on every edge, as chrono's `Serialize` writes it — `Z` at
/// offset zero, the fraction to the precision held — and on GraphQL the
/// `DateTime` scalar a write's input already takes.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(transparent)]
pub struct WireDateTime(DateTime<FixedOffset>);

impl<Tz: TimeZone> From<&DateTime<Tz>> for WireDateTime {
    fn from(at: &DateTime<Tz>) -> Self {
        Self(at.fixed_offset())
    }
}

// By hand: the derive's `transparent` gives way to a documented type, whose
// schema would then carry this rustdoc into every document and be referenced.
impl JsonSchema for WireDateTime {
    fn inline_schema() -> bool {
        <DateTime<FixedOffset> as JsonSchema>::inline_schema()
    }

    fn schema_name() -> Cow<'static, str> {
        <DateTime<FixedOffset> as JsonSchema>::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        <DateTime<FixedOffset> as JsonSchema>::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        <DateTime<FixedOffset> as JsonSchema>::json_schema(generator)
    }
}

#[cfg(feature = "graphql")]
mod graphql {
    use async_graphql::{
        Description, InputValueError, InputValueResult, Scalar, ScalarType, Value,
    };
    use chrono::{DateTime, FixedOffset, SecondsFormat};

    use super::WireDateTime;

    // A schema prints the first `DateTime` it registers — this one, or the
    // async-graphql one a write's input carries — so this one says its words.
    impl Description for WireDateTime {
        fn description() -> &'static str {
            "Implement the DateTime<FixedOffset> scalar\n\nThe input/output is a string in RFC3339 format."
        }
    }

    #[Scalar(
        name = "DateTime",
        specified_by_url = "https://datatracker.ietf.org/doc/html/rfc3339",
        use_type_description
    )]
    impl ScalarType for WireDateTime {
        fn parse(value: Value) -> InputValueResult<Self> {
            match &value {
                Value::String(written) => Ok(Self(written.parse::<DateTime<FixedOffset>>()?)),
                _ => Err(InputValueError::custom("expected an RFC 3339 date-time")),
            }
        }

        fn to_value(&self) -> Value {
            Value::String(self.0.to_rfc3339_opts(SecondsFormat::AutoSi, true))
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, Utc};

    use super::*;

    /// Instants at offset zero and away from it, each precision chrono writes,
    /// and a leap second.
    fn corpus() -> Vec<DateTime<FixedOffset>> {
        let day = NaiveDate::from_ymd_opt(2026, 10, 8).expect("a date");
        let east = FixedOffset::east_opt(2 * 3600).expect("an offset");
        let mut corpus = Vec::new();
        for nano in [0, 250_000_000, 250_123_000, 250_123_456, 1_500_000_000] {
            let at = day.and_hms_nano_opt(12, 30, 59, nano).expect("a time");
            corpus.push(Utc.from_utc_datetime(&at).fixed_offset());
            corpus.push(east.from_utc_datetime(&at));
        }
        corpus
    }

    #[test]
    fn an_instant_serializes_as_chrono_spells_it() {
        for at in corpus() {
            assert_eq!(
                serde_json::to_value(WireDateTime::from(&at)).expect("serializes"),
                serde_json::to_value(at).expect("serializes"),
            );
        }
        let at = corpus()[4];
        assert_eq!(
            serde_json::to_value(WireDateTime::from(&at)).expect("serializes"),
            "2026-10-08T12:30:59.250123Z",
        );
    }

    #[test]
    fn an_instant_keeps_its_offset_whatever_zone_its_column_holds() {
        let at = corpus()[3];
        let utc = WireDateTime::from(&at.with_timezone(&Utc));
        assert_eq!(
            serde_json::to_value(utc).expect("serializes"),
            "2026-10-08T12:30:59.250Z",
        );
        assert_eq!(
            serde_json::to_value(WireDateTime::from(&at)).expect("serializes"),
            "2026-10-08T14:30:59.250+02:00",
        );
    }

    #[test]
    fn the_schema_is_chronos_date_time() {
        let wire = schemars::schema_for!(WireDateTime);
        let chrono = schemars::schema_for!(DateTime<FixedOffset>);
        assert_eq!(wire, chrono);
        assert_eq!(wire.get("format"), Some(&"date-time".into()));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn graphql_writes_the_string_every_other_edge_writes() {
        use async_graphql::{ScalarType, Value};

        for at in corpus() {
            let serialized = serde_json::to_value(at).expect("serializes");
            let serde_json::Value::String(serialized) = serialized else {
                panic!("an instant serializes to a string");
            };
            assert_eq!(
                ScalarType::to_value(&WireDateTime::from(&at)),
                Value::String(serialized),
            );
        }
    }

    /// The day this fails, `WireDateTime` can go: `.claude/upstream/async-graphql.md`.
    #[cfg(feature = "graphql")]
    #[test]
    fn async_graphql_still_spells_offset_zero_plus_zero() {
        use async_graphql::{ScalarType, Value};

        let at = corpus()[2];
        assert_eq!(
            ScalarType::to_value(&at),
            Value::String("2026-10-08T12:30:59.250+00:00".to_owned()),
        );
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn graphql_reads_back_every_instant_it_writes() {
        use async_graphql::ScalarType;

        for at in corpus() {
            let wire = WireDateTime::from(&at);
            let read = <WireDateTime as ScalarType>::parse(ScalarType::to_value(&wire))
                .expect("its own spelling parses");
            assert_eq!(read.0, at);
        }
    }
}
