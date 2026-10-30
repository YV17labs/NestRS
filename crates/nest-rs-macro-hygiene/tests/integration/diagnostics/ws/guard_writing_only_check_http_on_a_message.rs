//! The WebSocket half of the same rule: a guard declaring that it checks
//! messages writes the decision a message runs, or it would pass every message
//! it is bound beside.

use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::ws::{WsView, gateway, messages};

#[injectable]
#[derive(Layer)]
struct RequestOnlyGuard;

#[async_trait]
impl Guard for RequestOnlyGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("refused at the request"))
    }
}

impl GuardFor<WsView> for RequestOnlyGuard {}

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("ping")]
    #[public]
    #[use_guards(RequestOnlyGuard)]
    async fn ping(&self) {}
}

fn main() {}
