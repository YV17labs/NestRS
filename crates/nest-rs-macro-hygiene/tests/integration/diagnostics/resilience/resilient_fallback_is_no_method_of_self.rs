//! `fallback` names a method of `Self`: one `Self` does not have is rustc's
//! own `E0599`, at the name written.

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

async fn stale(_error: ResilienceError<CarrierDown>) -> Answer {
    Ok(())
}

impl ShipmentsService {
    #[resilient(carrier, fallback = stale)]
    async fn ship(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
