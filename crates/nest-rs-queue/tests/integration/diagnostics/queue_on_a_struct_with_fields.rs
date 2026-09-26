//! A queue marker carries its identity in its type, so it has no state — a struct
//! with fields is refused naming the shape written.

use nest_rs_queue::queue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DemoCommand {
    id: String,
}

#[queue(name = "demo", job = DemoCommand)]
struct DemoQueue {
    name: String,
}

fn main() {}
