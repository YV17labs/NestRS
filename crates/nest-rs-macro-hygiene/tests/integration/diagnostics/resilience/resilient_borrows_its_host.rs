//! A decorated method borrows its host, as every impl half's does: no
//! receiver, `self` and `&mut self` are refused quoting the one written, and
//! the body, written as answering `Result<T, E>`, reports nothing more —
//! nor does one reading the token its `cancel` names, under any key.

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

    #[resilient(carrier, cancel = token)]
    async fn detached_reading_its_token() -> Answer {
        if token.is_cancelled() {
            return Ok(());
        }
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, cancel = token)]
    async fn consumed_reading_its_token(self) -> Answer {
        if token.is_cancelled() {
            return Ok(());
        }
        Err(CarrierDown)
    }

    async fn stale(&self, _error: ResilienceError<CarrierDown>) -> Answer {
        Ok(())
    }

    #[resilient(carrier, fallback = stale, cancel = token)]
    async fn exclusive_reading_its_token(&mut self) -> Answer {
        if token.is_cancelled() {
            return Ok(());
        }
        Err(CarrierDown)
    }
}

fn main() {}
