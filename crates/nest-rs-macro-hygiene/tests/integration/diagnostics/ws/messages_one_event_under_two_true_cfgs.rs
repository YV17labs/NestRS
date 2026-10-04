//! Two declarations of one event whose `#[cfg]`s both hold are two dispatches of
//! one event. Refused, because the dispatch arm and the guard chain would
//! disagree on the winner: the `match` keeps the first method, the chain table
//! keeps the second method's guards, so the first ran under the second's chain.

use nest_rs::core::{Layer, injectable};
use nest_rs::guards::{Denial, Guard, WsGuard, async_trait};
use nest_rs::ws::{WsClient, gateway, messages};

#[injectable]
#[derive(Default)]
struct Deny;

impl Layer for Deny {}

#[async_trait]
impl Guard for Deny {
    async fn check_ws_message(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &nest_rs::ws::serde_json::Value,
    ) -> Result<(), Denial> {
        Err(Denial::forbidden("never"))
    }
}

impl WsGuard for Deny {}

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[cfg(all())]
    #[subscribe_message("ping")]
    #[use_guards(Deny)]
    #[public]
    async fn guarded(&self) {}

    #[cfg(all())]
    #[subscribe_message("ping")]
    #[public]
    async fn open(&self) {}
}

fn main() {}
