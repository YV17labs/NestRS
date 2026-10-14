//! Two declarations of one event whose `#[cfg]`s both hold are two dispatches of
//! one event. Refused, because the dispatch arm and the guard chain would
//! disagree on the winner: the `match` keeps the first method, the chain table
//! keeps the second method's guards, so the first ran under the second's chain.

use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::ws::{WsView, gateway, messages};

#[injectable]
#[derive(Default)]
struct Deny;

impl Layer for Deny {}

#[async_trait]
impl Guard for Deny {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("never"))
    }
}

impl GuardFor<WsView> for Deny {}

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
