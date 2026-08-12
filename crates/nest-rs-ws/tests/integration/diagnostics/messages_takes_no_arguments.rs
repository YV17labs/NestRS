//! The impl half collects; it declares nothing. `#[messages]` took an argument
//! list and dropped it — including `version`, which the `#[gateway]` one line up
//! genuinely declares.

use nest_rs_ws::{gateway, messages};

#[gateway(path = "/ws")]
struct ChatGateway;

#[messages(version = "1")]
impl ChatGateway {}

fn main() {}
