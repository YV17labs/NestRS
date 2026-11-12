//! `#[layer]` reads one key, `priority`, through the shared grammar: another
//! is refused naming what it takes, and a second `#[layer]` is refused rather
//! than read over the first.

use nest_rs::core::Layer;

#[derive(Layer)]
#[layer(order = 1)]
struct Misnamed;

#[derive(Layer)]
#[layer(priority = 1)]
#[layer(priority = 2)]
struct WrittenTwice;

fn main() {}
