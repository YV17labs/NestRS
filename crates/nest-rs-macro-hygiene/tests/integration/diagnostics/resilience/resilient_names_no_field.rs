//! `#[resilient]` runs a method under the policy one field of `self` holds, so
//! it names that field: written with none, it says how rather than guessing one.

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
    #[resilient]
    async fn ship(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient()]
    async fn track(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
