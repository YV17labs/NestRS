//! The policy is read as `self.<field>`, so a field the host does not have is
//! rustc's `E0609` at the name the attribute wrote, its help offering the field
//! meant. This fixture deliberately fails to resolve: the name is the subject.

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
    #[resilient(carier)]
    async fn ship(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
