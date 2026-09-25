//! One event dispatches to one method: a second `#[subscribe_message]` for it is
//! refused naming both, rather than compiled to an arm that never runs.

use nest_rs_ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}

    #[subscribe_message("ping")]
    #[public]
    async fn pong(&self) {}
}

fn main() {}
