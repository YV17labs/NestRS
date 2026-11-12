//! [`QueueName`] — a queue's wire name, checked.

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::QueueError;

/// A queue's wire name: 1 to [`MAX_LEN`](Self::MAX_LEN) of `[A-Za-z0-9_.-]`,
/// checked when the value is made — at compile time for a `#[queue]` literal, at
/// boot for a `#[process]` method's queue, at the push for a raw name.
///
/// No `:`, a datastore key's level separator, and no whitespace or control
/// character (a log field, a metric label). A backend with a narrower rule
/// refuses what it cannot file, naming its own fact.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct QueueName(Cow<'static, str>);

impl QueueName {
    /// The longest a queue name may be.
    pub const MAX_LEN: usize = 128;

    /// The queue named `name` — a `#[queue]`'s literal, or a name read back from
    /// a backend's record or from the raw push hatch.
    pub fn new(name: impl Into<Cow<'static, str>>) -> Result<Self, QueueError> {
        let name = name.into();
        match refusal(&name) {
            None => Ok(Self(name)),
            Some(reason) => Err(QueueError::InvalidQueueName { reason }),
        }
    }

    /// The whole wire name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `value` follows the rule a queue name follows; public so the
/// decorator's compile-time copy of the rule is pinned to it.
pub fn is_valid_queue_name(value: &str) -> bool {
    refusal(value).is_none()
}

/// Which part of the rule `value` breaks, if any.
fn refusal(value: &str) -> Option<&'static str> {
    if value.is_empty() {
        Some("it is empty")
    } else if value.len() > QueueName::MAX_LEN {
        Some("it is too long")
    } else if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        Some("it holds a character no queue name takes")
    } else {
        None
    }
}

impl fmt::Display for QueueName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for QueueName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("QueueName").field(&self.as_str()).finish()
    }
}

/// The wire name, as [`as_str`](QueueName::as_str) spells it — how a
/// [`PushReceipt`](crate::PushReceipt) kept by a caller carries its queue.
impl Serialize for QueueName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Read back through [`new`](QueueName::new), so a stored name is checked again
/// rather than trusted.
impl<'de> Deserialize<'de> for QueueName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_inside_the_rule_is_its_own_wire_name() {
        let name = QueueName::new("audio.preview").expect("valid");
        assert_eq!(name.as_str(), "audio.preview");
        assert!(QueueName::new("a".repeat(QueueName::MAX_LEN)).is_ok());
    }

    #[test]
    fn a_name_outside_the_rule_is_refused_whatever_part_of_it_is_wrong() {
        let too_long = "a".repeat(QueueName::MAX_LEN + 1);
        for refused in [
            "",
            "nestrs:queue:dead",
            "with space",
            "line\nbreak",
            "tenant#acme",
            too_long.as_str(),
        ] {
            assert!(QueueName::new(refused.to_owned()).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn a_name_round_trips_through_serde_and_is_checked_on_the_way_back() {
        let name = QueueName::new("audio").expect("valid");
        let json = serde_json::to_value(&name).expect("serializes");
        assert_eq!(json, serde_json::Value::String(name.as_str().to_owned()));
        let back: QueueName = serde_json::from_value(json).expect("deserializes");
        assert_eq!(back, name);
        assert!(serde_json::from_value::<QueueName>(serde_json::json!("a:b")).is_err());
    }

    #[test]
    fn a_refused_name_is_said_by_the_part_of_the_rule_it_breaks() {
        for (refused, reason) in [
            (String::new(), "it is empty"),
            ("x".repeat(400), "it is too long"),
            (
                "tenant#acme".to_owned(),
                "it holds a character no queue name takes",
            ),
        ] {
            let error = QueueName::new(refused).expect_err("refused");
            assert!(
                matches!(error, QueueError::InvalidQueueName { reason: said } if said == reason),
                "{error:?}"
            );
        }
    }
}
