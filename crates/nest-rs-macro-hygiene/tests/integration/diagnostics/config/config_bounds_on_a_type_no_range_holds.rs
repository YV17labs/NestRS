//! A range holds a `Duration`, an `Option<Duration>` or a `usize`: bounds on
//! any other type is a compile error naming the three, never a value read with
//! its unit lost.

use nest_rs::config::{Bound, DurationBounds, Floor, config};

const WINDOW: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(1, "a shorter window counts nothing")),
    Bound::new(3600, "past an hour it is a unit slip"),
);

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig {
    #[config(bounds = WINDOW)]
    window: u64,
}

fn main() {}
