//! No mask shapes what a connection hook sends, so it takes no posture: a read under
//! the caller's rules belongs in a message whose posture names it.

use nest_rs::ws::{gateway, messages};

#[gateway(path = "/ws")]
struct LobbyGateway;

#[messages]
impl LobbyGateway {
    #[on_connect]
    #[public]
    async fn joined(&self) {}

    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        "pong".to_owned()
    }
}

fn main() {}
