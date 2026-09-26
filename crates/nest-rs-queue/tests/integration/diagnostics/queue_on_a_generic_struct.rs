//! A generic marker gives every instantiation one wire name, so two `Q<N>` would
//! claim one queue with nothing saying so — refused at the parameter.

use nest_rs_queue::queue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DemoCommand {
    id: String,
}

#[queue(name = "demo", job = DemoCommand)]
struct DemoQueue<const N: usize>;

fn main() {}
