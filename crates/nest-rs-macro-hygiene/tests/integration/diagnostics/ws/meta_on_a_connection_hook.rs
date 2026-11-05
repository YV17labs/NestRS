//! A connection hook takes no `#[meta]`: no layer runs around it, so nothing
//! would read the value, and accepting it would be a silent no-op.

use nest_rs::ws::{gateway, messages};

struct Audit;

#[gateway(path = "/ws")]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[on_connect]
    #[meta(Audit)]
    async fn joined(&self) {}

    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
