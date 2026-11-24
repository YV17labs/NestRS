//! Run once under `record_if`, the one attempt lends the arguments its
//! predicate may read once it answered, so an attempt writing through a `&mut`
//! the predicate reads is rustc's own `E0500`, at the `record_if` key that has
//! it lend them. A copy would keep the change from the caller: the predicate
//! reads the error and the other arguments, or an argument of its own.

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
    #[resilient(carrier, record_if = |_error| crates.is_empty())]
    async fn load(&self, crates: &mut Vec<String>) -> Answer<usize> {
        crates.push(String::new());
        Ok(crates.len())
    }
}

fn main() {}
