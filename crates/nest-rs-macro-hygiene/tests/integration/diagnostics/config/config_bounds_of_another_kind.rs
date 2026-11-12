//! A count is held to a `CountBounds`: a `DurationBounds` named on a `usize`
//! is a compile error at the field naming both, never a count read in seconds.

use nest_rs::config::{Bound, DurationBounds, Floor, config};

const LEASE: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(1, "a shorter lease renews in a loop")),
    Bound::new(3600, "past an hour it is a unit slip"),
);

#[config(namespace = "kiln")]
#[derive(Clone)]
struct KilnConfig {
    #[config(bounds = LEASE)]
    kilns: usize,
}

impl Default for KilnConfig {
    fn default() -> Self {
        Self { kilns: 2 }
    }
}

fn main() {}
