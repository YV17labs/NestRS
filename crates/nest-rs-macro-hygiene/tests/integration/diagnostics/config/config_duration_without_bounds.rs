//! A `Duration` field is read through the `DurationBounds` its range is
//! written in: one with no `#[config(bounds = …)]` is a compile error naming
//! the reader it needs, never a duration read with no floor or ceiling.

use std::time::Duration;

use nest_rs::config::config;

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig {
    window: Duration,
}

fn main() {}
