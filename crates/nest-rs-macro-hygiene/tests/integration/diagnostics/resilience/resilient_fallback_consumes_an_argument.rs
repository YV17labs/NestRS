//! Under `fallback` the attempt lends the arguments its fallback is handed
//! next, so an attempt consuming an owned one is rustc's own `E0382`, at the
//! `fallback` entry that hands it on, its help writing the clone.

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

async fn send(order: String) -> Result<usize, CarrierDown> {
    Ok(order.len())
}

impl ShipmentsService {
    async fn queued(&self, order: String, _error: ResilienceError<CarrierDown>) -> Answer<usize> {
        Ok(order.len())
    }

    #[resilient(carrier, fallback = queued)]
    async fn ship(&self, order: String) -> Answer<usize> {
        send(order).await
    }
}

fn main() {}
