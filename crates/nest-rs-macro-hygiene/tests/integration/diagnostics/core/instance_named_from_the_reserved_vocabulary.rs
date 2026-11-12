//! An instance's name is a namespace segment after the vendor, where a binding
//! puts its port: `Queue` would read `redis__queue`, the queue binding's own.

use nest_rs::core::instance;

#[instance]
pub struct Queue;

fn main() {}
