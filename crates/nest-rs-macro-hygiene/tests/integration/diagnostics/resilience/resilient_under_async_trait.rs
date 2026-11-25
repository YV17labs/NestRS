//! A method under `#[async_trait]` is a `fn` returning a boxed future by the
//! time `#[resilient]` reads it: refused as a `fn`, the sentence naming
//! `#[async_trait]` and the native `async fn` to write instead. Its body,
//! written as an attempt, answers that future's type and is kept out: the
//! refusal is the one error.

use std::sync::Arc;

use nest_rs::core::{async_trait, injectable};
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

#[async_trait]
trait Tracking {
    async fn track(&self, parcel: &str) -> Answer<String>;
    async fn locate(&self, parcel: &str) -> Answer<usize>;
}

#[async_trait]
impl Tracking for ShipmentsService {
    #[resilient(carrier, idempotent)]
    async fn track(&self, parcel: &str) -> Answer<String> {
        Ok(parcel.to_owned())
    }

    #[resilient(carrier, fallback = unlocated)]
    async fn locate(&self, parcel: &str) -> Answer<usize> {
        if parcel.is_empty() {
            return Err(CarrierDown);
        }
        let found: Result<usize, CarrierDown> = Ok(parcel.len());
        Ok(found?)
    }
}

impl ShipmentsService {
    async fn unlocated(
        &self,
        _parcel: &str,
        _error: ResilienceError<CarrierDown>,
    ) -> Answer<usize> {
        Ok(0)
    }
}

fn main() {}
