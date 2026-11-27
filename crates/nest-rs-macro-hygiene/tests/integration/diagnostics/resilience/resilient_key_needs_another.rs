//! `retry_if` and `retry_ok_if` say which answer is sent again, which a call
//! run once never is, and `fallback_if` which errors a fallback answers for:
//! each written without the key it needs is refused in one sentence naming
//! both, and adds no error for what its value is.

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
    async fn stale(&self, _error: ResilienceError<CarrierDown>) -> Answer {
        Ok(())
    }

    #[resilient(carrier, retry_if = |error| error.undefined())]
    async fn retried(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, retry_ok_if = undefined)]
    async fn polled(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback_if = |_| true)]
    async fn filtered(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, retries = 3, retry_if = |_| true, fallback_if = |_| true)]
    async fn several(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
