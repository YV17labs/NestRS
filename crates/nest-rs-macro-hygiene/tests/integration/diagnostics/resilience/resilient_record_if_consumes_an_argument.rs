//! Run once under a `record_if` naming an argument, the one attempt lends the
//! arguments its predicate may read once it answered, so an attempt consuming
//! one the predicate reads is rustc's own `E0505`, at the `record_if` key that
//! has it lend them; a clone in the body is the way out.

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
    #[resilient(carrier, record_if = |_error| !order.is_empty())]
    async fn ship(&self, order: String) -> Answer<usize> {
        send(order).await
    }
}

fn main() {}
