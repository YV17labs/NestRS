//! A `#[messages]` method borrows its gateway: `&mut self` is refused with that
//! fact, not with what rustc says of the dispatcher the expansion writes.

use nest_rs_ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&mut self) {}
}

fn main() {}
