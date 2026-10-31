//! A range holds the values between its floor and its ceiling: one whose floor
//! is above its ceiling holds none, and is a compile error in a `const`.

use nest_rs::config::{Bound, DurationBounds, Floor};

const WINDOW: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(60, "a shorter window counts nothing")),
    Bound::new(30, "past half a minute it is a unit slip"),
);

fn main() {
    let _ = WINDOW;
}
