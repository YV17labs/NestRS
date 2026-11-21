//! A gateway's connection hook answers `()`: the gateway calls it and moves on,
//! reading no answer. So no hook carries `#[resilient]` — one answering the
//! policy's `Result` is refused by `#[messages]`, one answering `()` by
//! `#[resilient]` — and a hook calls a decorated method instead, handling its
//! failure itself.

use std::sync::Arc;

use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};
use nest_rs::ws::{gateway, messages};

#[derive(Debug)]
struct CarrierDown;

impl std::fmt::Display for CarrierDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the carrier did not answer")
    }
}

impl std::error::Error for CarrierDown {}

#[gateway(path = "/ws/parcels")]
struct ParcelsGateway {
    #[inject]
    carrier: Arc<ResiliencePolicy>,
}

#[messages]
impl ParcelsGateway {
    #[on_connect]
    #[resilient(carrier)]
    async fn greet(&self) -> Result<(), ResilienceError<CarrierDown>> {
        Ok(())
    }

    #[on_disconnect]
    #[resilient(carrier)]
    async fn farewell(&self) {}
}

fn main() {}
