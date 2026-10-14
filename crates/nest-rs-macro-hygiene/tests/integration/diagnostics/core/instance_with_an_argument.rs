//! An instance's name is read off its ident, never chosen: a key written to
//! choose one is refused saying where the name comes from.

use nest_rs::core::instance;

#[instance(name = "cache")]
pub struct Cache;

fn main() {}
