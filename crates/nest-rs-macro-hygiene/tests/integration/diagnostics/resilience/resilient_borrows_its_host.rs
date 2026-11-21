//! A decorated method borrows its host, as every impl half's does: no
//! receiver, `self` and `&mut self` are refused quoting the one written, and
//! the body, written as answering `Result<T, E>`, reports nothing more.

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
    #[resilient(carrier)]
    async fn detached() -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier)]
    async fn consumed(self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier)]
    async fn exclusive(&mut self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
