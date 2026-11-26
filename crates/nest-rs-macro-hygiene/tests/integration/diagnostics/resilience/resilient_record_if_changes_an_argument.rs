//! Run once under a `record_if` naming an argument, the one attempt lends the
//! arguments its predicate may read once it answered, so an attempt changing
//! one it binds `mut` while the predicate reads it is rustc's own `E0502`, at
//! the `record_if` key that has it lend them; the way out is no `mut` and a
//! copy changed in the body, the predicate reading the argument as it was
//! given.

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

async fn send(order: &str) -> Result<usize, CarrierDown> {
    Ok(order.len())
}

impl ShipmentsService {
    #[resilient(carrier, record_if = |_error| !order.is_empty())]
    async fn ship(&self, mut order: String) -> Answer<usize> {
        order.push('!');
        send(&order).await
    }
}

fn main() {}
