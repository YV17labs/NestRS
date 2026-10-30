//! The bounds a field names read that field: a `DurationBounds` whose key is
//! another field's is a compile error, never a variable read under a key the
//! field does not spell.

use std::time::Duration;

use nest_rs::config::{Bound, DurationBounds, Floor, config};

const LEASE: DurationBounds = DurationBounds::secs(
    "LEASE_SECS",
    "KilnConfig::lease",
    Floor::Units(Bound::new(1, "a shorter lease renews in a loop")),
    Bound::new(3600, "past an hour it is a unit slip"),
);

#[config(namespace = "kiln")]
#[derive(Clone)]
struct KilnConfig {
    #[config(bounds = LEASE)]
    window: Duration,
}

impl Default for KilnConfig {
    fn default() -> Self {
        Self { window: Duration::from_secs(60) }
    }
}

fn main() {}
