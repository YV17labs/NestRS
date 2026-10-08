//! [`JobId`] — the port's name for one job, minted when the job is pushed.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::QueueError;

/// The id a push gives a job: a UUID v7, written in its hyphenated string form.
///
/// The port mints it, and no backend does: a storage's own task id is reported
/// beside it as `backend_id`. Ids sort in push order.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(Uuid);

impl JobId {
    /// The id of a job pushed now.
    pub(crate) fn mint() -> Self {
        Self(Uuid::now_v7())
    }

    /// An id read back — from a receipt kept in a database, or from an envelope.
    ///
    /// Refused with [`QueueError::InvalidJobId`] unless `raw` is a version-7
    /// UUID.
    pub fn parse(raw: &str) -> Result<Self, QueueError> {
        match Uuid::try_parse(raw) {
            Ok(uuid) if uuid.get_version_num() == 7 => Ok(Self(uuid)),
            _ => Err(QueueError::InvalidJobId {
                id: raw.chars().take(QueueError::SHOWN_ID_LEN).collect(),
            }),
        }
    }

    /// The id's 16 bytes — what the retry jitter is derived from.
    pub(crate) fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// How long ago the id was minted — its job pushed — on this host's clock,
    /// read off the millisecond a UUID v7 carries; zero for an id minted ahead
    /// of the clock reading it.
    pub(crate) fn age(&self) -> Duration {
        let minted = self.0.get_timestamp().map_or(UNIX_EPOCH, |timestamp| {
            let (secs, nanos) = timestamp.to_unix();
            UNIX_EPOCH + Duration::new(secs, nanos)
        });
        SystemTime::now().duration_since(minted).unwrap_or_default()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.hyphenated(), f)
    }
}

impl fmt::Debug for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JobId({self})")
    }
}

impl Serialize for JobId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for JobId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_id_is_a_v7_and_reads_back_as_itself() {
        let id = JobId::mint();
        let written = id.to_string();
        assert_eq!(written.len(), 36, "the hyphenated form: {written}");
        assert_eq!(JobId::parse(&written).expect("a minted id parses"), id);
    }

    #[test]
    fn an_id_says_how_long_ago_it_was_minted() {
        assert!(JobId::mint().age() < Duration::from_secs(60));
        let old = JobId::parse("01890a5d-ac96-774b-bcce-b302099a8057").expect("a v7 id");
        assert!(old.age() > Duration::from_secs(365 * 24 * 60 * 60));
        let ahead = SystemTime::now() + Duration::from_secs(3600);
        let ahead = ahead.duration_since(UNIX_EPOCH).expect("after the epoch");
        let ahead = uuid::Uuid::new_v7(uuid::Timestamp::from_unix(
            uuid::NoContext,
            ahead.as_secs(),
            ahead.subsec_nanos(),
        ));
        assert_eq!(JobId(ahead).age(), Duration::ZERO);
    }

    #[test]
    fn ids_minted_one_after_another_sort_in_push_order() {
        let first = JobId::mint();
        let second = JobId::mint();
        assert!(first < second, "{first} then {second}");
    }

    #[test]
    fn an_id_no_push_could_have_minted_is_refused() {
        let v4 = "9f0c1f6e-1d2b-4c3a-8e7f-2a1b3c4d5e6f";
        for refused in ["", "job-0", v4, "01890a5d-ac96-774b-bcce-b302099a805"] {
            assert!(
                matches!(JobId::parse(refused), Err(QueueError::InvalidJobId { .. })),
                "{refused:?}",
            );
        }
    }

    #[test]
    fn a_refused_id_is_shown_truncated() {
        let error = JobId::parse(&"x".repeat(500)).expect_err("refused");
        let QueueError::InvalidJobId { id } = &error else {
            panic!("{error:?}");
        };
        assert_eq!(id.len(), QueueError::SHOWN_ID_LEN);
    }

    #[test]
    fn an_id_round_trips_through_serde_as_its_string_form() {
        let id = JobId::mint();
        let json = serde_json::to_value(&id).expect("serializes");
        assert_eq!(json, serde_json::Value::String(id.to_string()));
        let back: JobId = serde_json::from_value(json).expect("deserializes");
        assert_eq!(back, id);
        assert!(serde_json::from_value::<JobId>(serde_json::json!("job-0")).is_err());
    }
}
