//! `namespace` names the marker type whose `WsServer<N>` a gateway fans out on,
//! and a value that is not a type path is refused at itself, opening with the
//! decorator and the key — the sentence it had named the key alone.

use nest_rs::ws::{gateway, messages};

#[gateway(path = "/ws", namespace = "chat")]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
