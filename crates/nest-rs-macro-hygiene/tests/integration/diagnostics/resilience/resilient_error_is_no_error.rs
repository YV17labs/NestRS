//! The policy tells an error's chain in the line a retry files and in its
//! breaker's opening, so a decorated method's error is a `std::error::Error`,
//! sent once or again, whatever keys the method carries: one that is not is
//! one error at its return type, saying what to give the method instead —
//! a type of the app's own, `anyhow::Error` and a boxed error included.

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

    async fn held(
        &self,
        error: ResilienceError<Declined>,
    ) -> Result<(), ResilienceError<Declined>> {
        Err(error)
    }

    #[resilient(carrier, record_if = |_| true, fallback = held, cancel = token)]
    async fn weigh(&self) -> Result<(), ResilienceError<Declined>> {
        drop(token);
        Err(Declined)
    }

    #[resilient(carrier)]
    async fn quote(&self) -> Result<(), ResilienceError<anyhow::Error>> {
        Ok(())
    }

    #[resilient(carrier, idempotent)]
    async fn locate(
        &self,
    ) -> Result<(), ResilienceError<Box<dyn std::error::Error + Send + Sync>>> {
        Ok(())
    }
}

fn main() {}
