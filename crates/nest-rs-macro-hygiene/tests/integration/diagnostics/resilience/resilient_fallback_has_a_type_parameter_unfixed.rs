//! The fallback is called with the method's arguments, then the error, and
//! with nothing else: a type parameter of it those do not fix is rustc's own
//! `E0283` at the name written, whose help writes a type argument in the key,
//! which the key refuses saying what to name. One its arguments fix is a
//! fallback like any other.

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
    async fn stale<T: Default>(&self, _zone: &str, _error: ResilienceError<CarrierDown>) -> Answer {
        drop(T::default());
        Ok(())
    }

    async fn stale_for<Z: AsRef<str>>(
        &self,
        zone: Z,
        _error: ResilienceError<CarrierDown>,
    ) -> Answer {
        drop(zone.as_ref().len());
        Ok(())
    }

    #[resilient(carrier, fallback = stale)]
    async fn ship(&self, zone: &str) -> Answer {
        drop(zone.len());
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = stale::<u8>)]
    async fn ship_as_its_help_says(&self, zone: &str) -> Answer {
        drop(zone.len());
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = stale_for)]
    async fn ship_fixing_it(&self, zone: &str) -> Answer {
        drop(zone.len());
        Err(CarrierDown)
    }
}

fn main() {}
