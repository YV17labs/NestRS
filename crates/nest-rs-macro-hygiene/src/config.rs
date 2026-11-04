//! `#[config]` — its `Validate` derive must resolve through the framework's
//! `crate = ` override, never `::validator::` at the call site, and its overlay
//! through `nest_rs::config` alone, each reader a field's type or key chooses.

use std::time::Duration;

use nest_rs::config::{
    Bound, ConfigService, CountBounds, DurationBounds, Field, Floor, Result, config,
};
use nest_rs::pipes::validator::ValidationError;

/// The window's range, read under `WINDOW_SECS`.
const WINDOW: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(1, "a shorter window counts nothing")),
    Bound::new(3600, "past an hour it is a unit slip"),
);

/// The kilns' range, read under `KILNS`.
const KILNS: CountBounds = CountBounds::new(
    Bound::new(1, "a firing needs a kiln"),
    Bound::new(64, "a controller drives no more"),
);

/// A reader of its own, as `with = …` names one.
fn doubled(env: &ConfigService, field: Field, base: u32) -> Result<u32> {
    Ok(env.parse::<u32>(field.key())?.map_or(base, |read| read * 2))
}

/// A custom rule, naming `validator`'s error through the framework alone.
fn not_thirteen(retries: u32) -> std::result::Result<(), ValidationError> {
    if retries == 13 {
        return Err(ValidationError::new("unlucky"));
    }
    Ok(())
}

/// Minimal config, one field per reader.
#[config(namespace = "macro_hygiene")]
#[derive(Clone, Debug)]
pub struct MacroHygieneConfig {
    /// Validation rules, so the emitted `Validate` derive is exercised, a custom
    /// one with no `validator` line in this manifest.
    #[validate(range(min = 1), custom(function = "not_thirteen"))]
    pub retries: u32,
    /// Read through its bounds.
    #[config(bounds = WINDOW)]
    pub window: Option<Duration>,
    /// A count read through its bounds.
    #[config(bounds = KILNS)]
    pub kilns: usize,
    /// Read as JSON.
    #[config(json)]
    pub tags: Vec<String>,
    /// Read as a count, `0` unlimited.
    #[config(count)]
    pub batches: Option<usize>,
    /// Read through a function of its own.
    #[config(with = doubled)]
    pub heat: u32,
}

impl Default for MacroHygieneConfig {
    fn default() -> Self {
        Self {
            retries: 1,
            window: Some(Duration::from_secs(60)),
            kilns: 2,
            tags: Vec::new(),
            batches: None,
            heat: 1,
        }
    }
}
