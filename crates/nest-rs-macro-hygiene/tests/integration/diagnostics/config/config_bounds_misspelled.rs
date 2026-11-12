//! A range is a path the developer writes, so a misspelled one deliberately
//! fails to resolve: rustc names it where it is written, in the attribute,
//! with the range in scope as its suggestion — never at the field's type.

use std::time::Duration;

use nest_rs::config::{Bound, DurationBounds, Floor, config};

const WINDOW: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(1, "a shorter window counts nothing")),
    Bound::new(3600, "past an hour it is a unit slip"),
);

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig {
    #[config(bounds = WINDOWW)]
    lull: Duration,
}

fn main() {}
