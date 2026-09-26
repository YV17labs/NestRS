//! [`QueueName`] — a queue's wire name, checked.

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{QueueError, QueueKind};

/// What joins a dynamic queue's prefix to its key. Outside the name's charset,
/// so no static name can be read as an instance of a dynamic queue.
pub const INSTANCE_SEPARATOR: char = '#';

/// A queue's wire name: a static queue's name, or a dynamic queue's prefix and
/// key joined by [`INSTANCE_SEPARATOR`] (`tenant#acme`).
///
/// A name, a prefix and a key are each 1 to [`MAX_LEN`](Self::MAX_LEN) of
/// `[A-Za-z0-9_.-]`, checked
/// when the value is made — at compile time for a `#[queue]` literal, at boot
/// for a `#[process]` method's queue, at the push for a runtime key or a raw
/// name. The charset is what a name can carry through every surface it reaches
/// unescaped: no `:` — a **datastore key's level separator**, here and in every
/// store that spells one, which is why `nestrs:<concern>:<structure>` can hold a
/// queue name as a member at all — no `#` (the instance separator), no
/// whitespace or control character (a log field, a metric label). A backend with
/// a narrower rule refuses what it cannot file, naming its own fact.
///
/// `.` is permitted, and that is the line between a port's floor and a
/// backend's: it separates nothing here, and a broker that reads it as a subject
/// hierarchy is free to refuse a name carrying one.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct QueueName {
    name: Cow<'static, str>,
    /// Where the prefix ends, for an instance of a dynamic queue.
    prefix_len: Option<usize>,
}

impl QueueName {
    /// The longest a queue name, a dynamic queue's prefix or its key may be.
    pub const MAX_LEN: usize = 128;

    /// A static queue's name.
    pub fn new(name: impl Into<Cow<'static, str>>) -> Result<Self, QueueError> {
        let name = name.into();
        check(&name, "queue name")?;
        Ok(Self {
            name,
            prefix_len: None,
        })
    }

    /// The instance of the dynamic queue under `prefix` for `key`.
    pub fn instance(prefix: &str, key: &str) -> Result<Self, QueueError> {
        check(prefix, "dynamic queue prefix")?;
        check(key, "dynamic queue key")?;
        Ok(Self {
            name: Cow::Owned(format!("{prefix}{INSTANCE_SEPARATOR}{key}")),
            prefix_len: Some(prefix.len()),
        })
    }

    /// A name read back — from a backend's record, or from the raw push hatch:
    /// `prefix#key` for an instance, a static name otherwise.
    pub fn parse(raw: &str) -> Result<Self, QueueError> {
        match raw.split_once(INSTANCE_SEPARATOR) {
            Some((prefix, key)) => Self::instance(prefix, key),
            None => Self::new(raw.to_owned()),
        }
    }

    /// The whole wire name.
    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// The name a `#[process]` method declares: the static queue's name, or the
    /// dynamic queue's prefix.
    pub fn queue(&self) -> &str {
        match self.prefix_len {
            Some(end) => &self.name[..end],
            None => &self.name,
        }
    }

    /// The runtime key of a dynamic queue's instance; `None` for a static queue.
    pub fn instance_key(&self) -> Option<&str> {
        self.prefix_len
            .map(|end| &self.name[end + INSTANCE_SEPARATOR.len_utf8()..])
    }

    /// Whether the name is a static queue's or a dynamic queue's instance.
    pub fn kind(&self) -> QueueKind {
        match self.prefix_len {
            Some(_) => QueueKind::Dynamic,
            None => QueueKind::Static,
        }
    }

    /// Whether `value` follows the rule a name, a prefix and a key share.
    ///
    /// Public so the decorator's compile-time copy of the rule can be pinned
    /// against this one: the macro crate cannot depend on this crate.
    #[doc(hidden)]
    pub fn is_valid(value: &str) -> bool {
        !value.is_empty()
            && value.len() <= QueueName::MAX_LEN
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    }
}

impl fmt::Display for QueueName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
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

/// Read back through [`parse`](QueueName::parse), so a stored name is checked
/// again rather than trusted.
impl<'de> Deserialize<'de> for QueueName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Refuse `value` unless it follows the rule, naming what it was meant to be.
fn check(value: &str, what: &'static str) -> Result<(), QueueError> {
    if QueueName::is_valid(value) {
        return Ok(());
    }
    // Truncated before it reaches an error message a log line will carry: a raw
    // name is the caller's input and has no length limit of its own.
    let shown: String = value.chars().take(QueueName::MAX_LEN).collect();
    Err(QueueError::InvalidQueueName { name: shown, what })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_static_name_is_its_own_queue() {
        let name = QueueName::new("audio.preview").expect("valid");
        assert_eq!(name.as_str(), "audio.preview");
        assert_eq!(name.queue(), "audio.preview");
        assert_eq!(name.instance_key(), None);
        assert_eq!(name.kind(), QueueKind::Static);
    }

    #[test]
    fn an_instance_joins_its_prefix_and_key_and_reads_back_the_same() {
        let name = QueueName::instance("tenant", "acme-42").expect("valid");
        assert_eq!(name.as_str(), "tenant#acme-42");
        assert_eq!(name.queue(), "tenant");
        assert_eq!(name.instance_key(), Some("acme-42"));
        assert_eq!(name.kind(), QueueKind::Dynamic);
        assert_eq!(QueueName::parse("tenant#acme-42").expect("parses"), name);
    }

    #[test]
    fn a_name_outside_the_rule_is_refused_whatever_part_of_it_is_wrong() {
        let too_long = "a".repeat(QueueName::MAX_LEN + 1);
        for refused in [
            "",
            "nestrs:queue:dead",
            "with space",
            "line\nbreak",
            "tenant#",
            "#key",
            "a#b#c",
            too_long.as_str(),
        ] {
            assert!(QueueName::parse(refused).is_err(), "{refused:?}");
        }
        assert!(
            QueueName::new("tenant#acme").is_err(),
            "a static name has no `#`"
        );
        assert!(QueueName::new("a".repeat(QueueName::MAX_LEN)).is_ok());
    }

    #[test]
    fn a_name_round_trips_through_serde_and_is_checked_on_the_way_back() {
        for name in [
            QueueName::new("audio").expect("valid"),
            QueueName::instance("tenant", "acme").expect("valid"),
        ] {
            let json = serde_json::to_value(&name).expect("serializes");
            assert_eq!(json, serde_json::Value::String(name.as_str().to_owned()));
            let back: QueueName = serde_json::from_value(json).expect("deserializes");
            assert_eq!(back, name);
        }
        assert!(serde_json::from_value::<QueueName>(serde_json::json!("a:b")).is_err());
    }

    #[test]
    fn a_refused_name_is_shown_truncated_and_escaped() {
        let error = QueueName::new(format!("{}\u{7}", "x".repeat(400))).expect_err("refused");
        let rendered = error.to_string();
        assert!(rendered.len() < 400, "{rendered}");
        assert!(!rendered.contains('\u{7}'), "{rendered:?}");
    }
}
