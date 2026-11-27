//! A method's `retries` stand beside `idempotent` alone, a call run once never
//! being sent again, and within the range the policy's own are held to: `0`
//! says to drop `idempotent`, a count past ten is the config's refusal.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};

#[derive(Debug)]
struct CarrierDown;

impl std::fmt::Display for CarrierDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the carrier did not answer")
    }
}

impl std::error::Error for CarrierDown {}

type Answer<T = ()> = Result<T, ResilienceError<CarrierDown>>;

#[injectable]
struct ShipmentsService {
    #[inject]
    carrier: Arc<ResiliencePolicy>,
}

impl ShipmentsService {
    #[resilient(carrier, retries = 3)]
    async fn once(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, retries = 0)]
    async fn none(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, retries = 11)]
    async fn too_many(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, retries = "3")]
    async fn spelled(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
