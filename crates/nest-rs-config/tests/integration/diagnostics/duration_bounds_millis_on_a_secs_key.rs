//! `DurationBounds::millis` reads its value in milliseconds, so its key ends in
//! `_MS`: a key spelling another unit is a compile error in a `const`.

use nest_rs_config::{Bound, DurationBounds, Floor};

const WINDOW: DurationBounds = DurationBounds::millis(
    "WINDOW_SECS",
    "FixtureConfig::window",
    Floor::AboveZero("a zero window counts nothing"),
    Bound { count: 60_000, why: "past a minute it is a unit slip" },
);

fn main() {
    let _ = WINDOW;
}
