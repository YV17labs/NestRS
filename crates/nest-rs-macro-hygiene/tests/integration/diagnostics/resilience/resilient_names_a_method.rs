//! `#[resilient]` reads the policy from a field of `self`, never through a
//! method: a host answering its policy through one is rustc's own `E0615` at
//! the name written, whose help writes the call in the list, which the list
//! refuses saying to hold the policy in a field.

use std::sync::Arc;

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

struct ShipmentsClient {
    held: Arc<ResiliencePolicy>,
}

impl ShipmentsClient {
    fn carrier(&self) -> &Arc<ResiliencePolicy> {
        &self.held
    }

    #[resilient(carrier)]
    async fn ship(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier())]
    async fn ship_as_its_help_says(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(held)]
    async fn ship_under_its_field(&self) -> Answer {
        drop(self.carrier());
        Err(CarrierDown)
    }
}

fn main() {}
