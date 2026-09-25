//! A `#[messages]` method declares one role — a message, or one of the two
//! connection hooks. A message that is also `#[on_connect]` is refused naming
//! both, rather than served as whichever was read first.

use nest_rs_ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("ping")]
    #[on_connect]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
