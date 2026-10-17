//! The `tz` refusal, printed at compile time; the boot's copy of it is pinned
//! against this one by a test in `nest-rs-schedule`.

/// The refusal of a `#[cron]` `tz` the zone database does not hold — the
/// runtime's sentence with the site in front.
pub fn invalid_time_zone(value: &str) -> String {
    format!(
        "{}: {value:?} is not a zone of the IANA time zone database bundled with jiff — it \
         takes an `Area/Location` identifier (e.g. \"Europe/Paris\", \"America/New_York\", \
         \"UTC\")",
        crate::args::site("cron", Some("tz")),
    )
}
