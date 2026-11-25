//! `#[resilient]` runs a body under the policy a field of `self` holds, and a
//! trait declares a method with neither: on a declaration it says so at the
//! method's name, and where the decorator goes. A default body has `Self` for
//! a host, which has no field: rustc's own `E0609`, at the field named.

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

trait Tracking {
    #[resilient(carrier, idempotent)]
    async fn status(&self, parcel: &str) -> Answer<usize>;

    #[resilient(carrier)]
    async fn located(&self, parcel: &str) -> Answer<usize> {
        Ok(parcel.len())
    }
}

#[injectable]
struct ShipmentsService {
    #[inject]
    carrier: Arc<ResiliencePolicy>,
}

impl Tracking for ShipmentsService {
    #[resilient(carrier, idempotent)]
    async fn status(&self, parcel: &str) -> Answer<usize> {
        Ok(parcel.len())
    }
}

fn main() {}
