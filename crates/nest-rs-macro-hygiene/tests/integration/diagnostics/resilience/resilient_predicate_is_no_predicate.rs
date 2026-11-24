//! A predicate key's value that is no predicate in a way its shape does not
//! tell: a literal is the decorator's own refusal, at the value; a closure
//! whose body answers no `bool` is rustc's `E0308` at that body, one taking
//! two arguments its `E0593`; and a constant, which is not callable, is the
//! key's sentence at the value, then what rustc expected of the run.

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

const ALWAYS: bool = true;

impl ShipmentsService {
    #[resilient(carrier, idempotent, retry_if = true)]
    async fn flagged(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, record_if = |_error| 7)]
    async fn counted(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, retry_if = |_error, _attempt| true)]
    async fn numbered(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, record_if = ALWAYS)]
    async fn constant(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
