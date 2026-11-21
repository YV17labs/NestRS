//! A method under `#[async_trait]` is a `fn` returning a boxed future by the
//! time `#[resilient]` reads it: refused as a `fn`, the sentence naming
//! `#[async_trait]` and the native `async fn` to write instead.

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
}

#[async_trait]
impl Tracking for ShipmentsService {
    #[resilient(carrier, idempotent)]
    async fn track(&self, parcel: &str) -> Answer<String> {
        Ok(parcel.to_owned())
    }
}

fn main() {}
