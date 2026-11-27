//! The fallback answers what its method answers, from the error its run ended
//! on: one answering another `Ok`, taking another error, or that is no
//! `async fn`, is one error, at the name written, in the key's sentence.

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

#[derive(Debug)]
struct DepotDown;

impl std::fmt::Display for DepotDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the depot did not answer")
    }
}

impl std::error::Error for DepotDown {}

impl ShipmentsService {
    async fn another_answer(&self, _error: ResilienceError<CarrierDown>) -> Answer<String> {
        Ok(String::new())
    }

    async fn another_error(&self, _error: ResilienceError<DepotDown>) -> Answer<usize> {
        Ok(0)
    }

    fn no_async_fn(&self, _error: ResilienceError<CarrierDown>) -> Answer<usize> {
        Ok(0)
    }

    #[resilient(carrier, fallback = another_answer)]
    async fn ship(&self) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = another_error)]
    async fn weigh(&self) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = no_async_fn)]
    async fn label(&self) -> Answer<usize> {
        Err(CarrierDown)
    }
}

fn main() {}
