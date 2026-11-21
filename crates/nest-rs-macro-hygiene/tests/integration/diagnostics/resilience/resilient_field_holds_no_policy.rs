//! The field a decorated method names holds its policy, an
//! `Arc<ResiliencePolicy<I>>`: one holding anything else, the policy by value
//! included, is one error at the name the attribute wrote.

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
    carrier: Arc<String>,
}

impl ShipmentsService {
    #[resilient(carrier)]
    async fn ship(&self) -> Answer {
        Err(CarrierDown)
    }
}

struct HandBuilt {
    carrier: ResiliencePolicy,
}

impl HandBuilt {
    #[resilient(carrier, idempotent)]
    async fn track(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
