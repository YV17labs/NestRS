//! `#[queue]` gives a unit struct a queue's identity. On an impl block it is
//! refused naming what it was written on, rather than with syn's `expected
//! struct`.

use nest_rs_queue::queue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DemoCommand {
    id: String,
}

struct DemoQueue;

#[queue(name = "demo", job = DemoCommand)]
impl DemoQueue {}

fn main() {}
