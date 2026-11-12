//! A connection hook takes no guard: it runs on a socket its upgrade's guards
//! admitted, so the gate is the `#[gateway]`'s `#[use_guards]`.

use nest_rs::ws::{gateway, messages};

struct AllowAll;

#[gateway(path = "/ws")]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[on_connect]
    #[use_guards(AllowAll)]
    async fn joined(&self) {}

    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
