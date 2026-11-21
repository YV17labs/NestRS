//! A retry reads the error's chain for a `Retry-After`, so an idempotent
//! method's error is a `std::error::Error`: one that is not is one error at its
//! return type, where a method sent once takes it as it is.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};

#[derive(Debug)]
struct Declined;

#[injectable]
struct ShipmentsService {
    #[inject]
    carrier: Arc<ResiliencePolicy>,
}

impl ShipmentsService {
    #[resilient(carrier)]
    async fn ship(&self) -> Result<(), ResilienceError<Declined>> {
        Err(Declined)
    }

    #[resilient(carrier, idempotent)]
    async fn track(&self) -> Result<(), ResilienceError<Declined>> {
        Err(Declined)
    }
}

fn main() {}
