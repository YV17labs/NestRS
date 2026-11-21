//! A key `#[resilient]` does not take, one written twice, one written before
//! the field, a field written as an expression, `idempotent` given a value and
//! a pin given none: each refused at what was written, in the grammar's words.

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
    #[resilient(carrier, retry = 3)]
    async fn misspelled(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, timeout = "1s", timeout = "2s")]
    async fn twice(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(idempotent, carrier)]
    async fn keyed_first(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(self.carrier)]
    async fn expression(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent = true)]
    async fn flagged(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, timeout)]
    async fn bare(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
