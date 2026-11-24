//! The policy tells an error's chain in the line a retry files and in its
//! breaker's opening, so a decorated method's error is a `std::error::Error`,
//! sent once or again: one that is not is one error at its return type.

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
