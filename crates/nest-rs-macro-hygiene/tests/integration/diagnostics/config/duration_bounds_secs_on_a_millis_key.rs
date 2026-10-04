//! `DurationBounds::secs` reads its value in seconds, so its key ends in
//! `_SECS`: a key spelling another unit is a compile error in a `const`, never a
//! variable whose name and value disagree by a factor of a thousand.

use nest_rs::config::{Bound, DurationBounds, Floor};

const DEADLINE: DurationBounds = DurationBounds::secs(
    "DEADLINE_MS",
    "FixtureConfig::deadline",
    Floor::AboveZero("a zero deadline answers nothing"),
    Bound { count: 60, why: "past a minute it is a unit slip" },
);

fn main() {
    let _ = DEADLINE;
}
