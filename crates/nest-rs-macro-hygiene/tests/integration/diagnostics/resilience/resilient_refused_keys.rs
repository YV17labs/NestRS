//! A key `#[resilient]` does not take, one written twice, before the field or
//! after it with no comma, `idempotent` written alone, which a field may be
//! named, a field written as an expression, as a call or as a tuple struct's
//! position, `idempotent` given a value, a pin given none or one that reads as
//! no expression: each refused where it is written, in the decorator's words,
//! and the keys around a refused one read.

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

    #[resilient(idempotent)]
    async fn keyed_alone(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier idempotent, timeout = "2s")]
    async fn no_comma(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(self.carrier)]
    async fn expression(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier())]
    async fn called(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(0)]
    async fn positional(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, retries =)]
    async fn valueless(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, retries = =)]
    async fn no_expression(&self) -> Answer {
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

    #[resilient(carrier, retries = 11, retry = 3, idempotent, timeout)]
    async fn several(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, retries = 3, retry = 3)]
    async fn once(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
