use sea_orm::prelude::DateTimeWithTimeZone;

/// The current instant as a timezone-aware timestamp, ready to store in a
/// `DateTimeWithTimeZone` column. Centralizes the `Utc::now().fixed_offset()`
/// dance so callers (and [`Repo`](crate::Repo) itself) never open-code it.
pub fn now() -> DateTimeWithTimeZone {
    chrono::Utc::now().fixed_offset()
}
