//! [`Attribute`] — a key and a value a measurement is recorded under.

use std::borrow::Cow;
use std::sync::Arc;

use opentelemetry::{Key, KeyValue, StringValue, Value};

/// A key and a value a measurement is recorded under — one dimension of the
/// series it adds to (OpenTelemetry's attribute).
///
/// ```
/// use nest_rs_opentelemetry::Attribute;
///
/// let source = Attribute::new("source", "api");
/// let retried = Attribute::new("retried", true);
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Attribute {
    /// Built once, here: recording clones it, which never allocates.
    pub(crate) key_value: KeyValue,
}

impl Attribute {
    /// `key` with `value`.
    pub fn new(key: &'static str, value: impl Into<AttributeValue>) -> Self {
        let value = match value.into() {
            AttributeValue::Bool(value) => Value::Bool(value),
            AttributeValue::I64(value) => Value::I64(value),
            AttributeValue::F64(value) => Value::F64(value),
            AttributeValue::String(Cow::Borrowed(text)) => Value::String(StringValue::from(text)),
            AttributeValue::String(Cow::Owned(text)) => {
                Value::String(StringValue::from(Arc::<str>::from(text)))
            }
        };
        Self {
            key_value: KeyValue::new(Key::from_static_str(key), value),
        }
    }
}

/// The value of an [`Attribute`], one of the primitive types the
/// OpenTelemetry attribute model holds.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum AttributeValue {
    /// A boolean.
    Bool(bool),
    /// A signed integer.
    I64(i64),
    /// A floating-point number.
    F64(f64),
    /// A string.
    String(Cow<'static, str>),
}

impl From<bool> for AttributeValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for AttributeValue {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

impl From<f64> for AttributeValue {
    fn from(value: f64) -> Self {
        Self::F64(value)
    }
}

impl From<&'static str> for AttributeValue {
    fn from(value: &'static str) -> Self {
        Self::String(Cow::Borrowed(value))
    }
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        Self::String(Cow::Owned(value))
    }
}

impl From<Cow<'static, str>> for AttributeValue {
    fn from(value: Cow<'static, str>) -> Self {
        Self::String(value)
    }
}

/// `attributes` as the SDK reads them, handed to `record` — on the stack up
/// to eight, the common case, so a measurement allocates nothing of its own.
pub(crate) fn with_key_values(attributes: &[Attribute], record: impl FnOnce(&[KeyValue])) {
    fn on_stack<const N: usize>(attributes: &[Attribute], record: impl FnOnce(&[KeyValue])) {
        let key_values: [KeyValue; N] =
            std::array::from_fn(|index| attributes[index].key_value.clone());
        record(&key_values);
    }
    match attributes.len() {
        0 => record(&[]),
        1 => on_stack::<1>(attributes, record),
        2 => on_stack::<2>(attributes, record),
        3 => on_stack::<3>(attributes, record),
        4 => on_stack::<4>(attributes, record),
        5 => on_stack::<5>(attributes, record),
        6 => on_stack::<6>(attributes, record),
        7 => on_stack::<7>(attributes, record),
        8 => on_stack::<8>(attributes, record),
        _ => record(
            &attributes
                .iter()
                .map(|attribute| attribute.key_value.clone())
                .collect::<Vec<_>>(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handed(attributes: &[Attribute]) -> Vec<String> {
        let mut seen = Vec::new();
        with_key_values(attributes, |key_values| {
            seen = key_values
                .iter()
                .map(|kv| format!("{}={}", kv.key, kv.value))
                .collect();
        });
        seen
    }

    #[test]
    fn every_attribute_is_handed_in_order_whatever_the_count() {
        for count in 0..=10_i64 {
            let attributes: Vec<Attribute> = (0..count).map(|n| Attribute::new("n", n)).collect();
            let expected: Vec<String> = (0..count).map(|n| format!("n={n}")).collect();
            assert_eq!(handed(&attributes), expected, "{count} attributes");
        }
    }
}
