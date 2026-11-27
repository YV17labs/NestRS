//! `#[resilient]` decorates a method: on anything else it says where it goes.

use std::sync::Arc;

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

#[resilient(carrier)]
struct ShipmentsService {
    carrier: Arc<ResiliencePolicy>,
}

fn main() {}
