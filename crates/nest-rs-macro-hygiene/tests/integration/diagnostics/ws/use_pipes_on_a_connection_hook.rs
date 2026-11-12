//! A connection hook takes no pipe: it is handed no payload for one to read.

use nest_rs::ws::{gateway, messages};

struct Trim;

#[gateway(path = "/ws")]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[on_connect]
    #[use_pipes(Trim)]
    async fn joined(&self) {}

    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
