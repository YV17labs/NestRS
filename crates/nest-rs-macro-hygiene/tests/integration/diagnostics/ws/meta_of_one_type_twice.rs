//! Two `#[meta]` values of one type on one gateway: refused at the second.

use nest_rs::ws::{gateway, messages};

struct Limit(u32);

#[gateway(path = "/chat")]
#[meta(Limit(10))]
#[meta(Limit(20))]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
