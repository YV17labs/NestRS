//! A gateway owns its mount outright, so there is no second path for a second
//! version to answer at. The list spelling `#[controller]` accepts is refused
//! here by a sentence naming that fact — not by a bare "expected a string
//! literal", which is what a second parser produced.

use nest_rs_ws::gateway;

#[gateway(path = "/ws", version = ["1", "2"])]
struct ChatGateway;

fn main() {}
