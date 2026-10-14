//! An instance is a marker: `#[instance]` on a struct carrying fields says the
//! marker holds nothing, and where the value it would hold belongs.

use nest_rs::core::instance;

#[instance]
pub struct Cache {
    pub url: String,
}

fn main() {}
