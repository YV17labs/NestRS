//! A method's `timeout` is a duration literal within the range the policy's
//! own is held to, its refusal the config's with the site in front.

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
    #[resilient(carrier, timeout = 2000)]
    async fn unitless(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, timeout = "11m")]
    async fn too_long(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
