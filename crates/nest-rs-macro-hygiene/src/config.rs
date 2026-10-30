//! `#[config]` — its `Validate` derive must resolve through the framework's
//! `crate = ` override, never `::validator::` at the call site, and its overlay
//! through `nest_rs::config` alone, each reader a field's type or key chooses.

use std::time::Duration;

use nest_rs::config::{Bound, ConfigService, DurationBounds, Floor, Result, config};

/// The window's range, read under `WINDOW_SECS`.
const WINDOW: DurationBounds = DurationBounds::secs(
    "WINDOW_SECS",
    "MacroHygieneConfig::window",
    Floor::UnitsOrOff(Bound::new(1, "a shorter window counts nothing")),
    Bound::new(3600, "past an hour it is a unit slip"),
);

/// A reader of its own, as `with = …` names one.
fn doubled(env: &ConfigService, key: &str, base: u32) -> Result<u32> {
    Ok(env.parse::<u32>(key)?.map_or(base, |read| read * 2))
}

/// Minimal config, one field per reader.
#[config(namespace = "macro_hygiene", defaults = Self::profile_defaults)]
#[derive(Clone, Debug)]
pub struct MacroHygieneConfig {
    /// A validation rule, so the emitted `Validate` derive is exercised.
    #[validate(range(min = 1))]
    pub retries: u32,
    /// Read through its bounds.
    #[config(bounds = WINDOW)]
    pub window: Option<Duration>,
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

impl MacroHygieneConfig {
    /// The base the derived `Config::defaults` returns.
    fn profile_defaults() -> Self {
        Self::default()
    }
}

impl Default for MacroHygieneConfig {
    fn default() -> Self {
        Self {
            retries: 1,
            window: Some(Duration::from_secs(60)),
            tags: Vec::new(),
            batches: None,
            heat: 1,
        }
    }
}
