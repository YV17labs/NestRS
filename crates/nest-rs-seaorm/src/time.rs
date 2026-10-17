use sea_orm::prelude::DateTimeWithTimeZone;

/// The current instant as a timezone-aware timestamp, ready to store in a
/// `DateTimeWithTimeZone` column.
pub fn now() -> DateTimeWithTimeZone {
    chrono::Utc::now().fixed_offset()
}
