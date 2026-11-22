//! A pin for a setting the policy holds for every caller — the backoff's, the
//! breaker's, the bulkhead's and the rate limit's — refused at each key, told
//! which mechanism shares it and where it is set: a method pins its `timeout`
//! and its `retries` alone.

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
    #[resilient(carrier, idempotent, backoff_first = "100ms")]
    async fn spaced(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, backoff_ceiling = "5s")]
    async fn capped(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, breaker_window = 20)]
    async fn windowed(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, breaker_failures = 5)]
    async fn tripped(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, breaker_open = "30s")]
    async fn opened(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, breaker_age = "60s")]
    async fn aged(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, breaker_probes = 3)]
    async fn probed(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, concurrency = 4)]
    async fn bounded(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, concurrency_wait = "1s")]
    async fn queued(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, rate_limit = 5)]
    async fn limited(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, rate_window = "1s")]
    async fn counted(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, rate_wait = "100ms")]
    async fn waiting(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
