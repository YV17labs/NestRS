//! A `#[module]` key written twice is refused, as it is at every other member of
//! the `key = value` family.
//!
//! It merged the two lists, so nothing was dropped — which is why this is not
//! the same defect `duplicate_argument` was written for. It is the other one: a
//! grammar the framework interprets accepting a spelling its siblings reject,
//! with no sentence and no cell. One list is what a reader can see whole.

use nest_rs_core::{injectable, module};

#[injectable]
#[derive(Default)]
struct A;

#[injectable]
#[derive(Default)]
struct B;

#[module(providers = [A], providers = [B])]
struct AppModule;

fn main() {}
