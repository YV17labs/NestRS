//! The fallback is handed the method's arguments, in order, then the run's
//! error: one taking another type, or fewer arguments, is rustc's own `E0308`
//! or `E0061`, at the `fallback` key, its note showing the fallback's signature.

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
    async fn by_reference(&self, order: &str, _error: ResilienceError<CarrierDown>) -> Answer<usize> {
        Ok(order.len())
    }

    async fn the_error_alone(&self, _error: ResilienceError<CarrierDown>) -> Answer<usize> {
        Ok(0)
    }

    #[resilient(carrier, fallback = by_reference)]
    async fn ship(&self, order: String) -> Answer<usize> {
        Ok(order.len())
    }

    #[resilient(carrier, fallback = the_error_alone)]
    async fn weigh(&self, order: String) -> Answer<usize> {
        Ok(order.len())
    }
}

fn main() {}
