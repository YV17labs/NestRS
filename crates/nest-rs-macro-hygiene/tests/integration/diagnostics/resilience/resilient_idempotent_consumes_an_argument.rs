//! `idempotent` reads the arguments again at every attempt, so an attempt
//! consuming an owned one is rustc's own `E0507`, pointed at the `idempotent`
//! that declared the replay, its help writing the clone.

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

async fn send(order: String) -> Result<(), CarrierDown> {
    drop(order);
    Ok(())
}

impl ShipmentsService {
    #[resilient(carrier, idempotent)]
    async fn ship(&self, order: String) -> Answer {
        send(order).await?;
        Ok(())
    }
}

fn main() {}
