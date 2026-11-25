//! A method runs under one policy: a second `#[resilient]` is refused at the
//! copy, which would run the first's whole run again at each of its attempts —
//! read by its name where it is written beside the first, and by the mark the
//! first left in the method's body where its name does not show it: imported
//! under another, an entry of a `#[cfg_attr]`, or below an instrument. A first
//! one refused leaves it too, and both refusals are told at once.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::resilience::resilient as shielded;
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
    #[resilient(carrier)]
    #[resilient(carrier, idempotent)]
    async fn ship(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier)]
    #[shielded(carrier, idempotent)]
    async fn under_another_name_below(&self) -> Answer {
        Err(CarrierDown)
    }

    #[shielded(carrier)]
    #[resilient(carrier, idempotent)]
    async fn under_another_name_above(&self) -> Answer {
        Err(CarrierDown)
    }

    #[shielded(carrier)]
    #[shielded(carrier, idempotent)]
    async fn under_another_name_twice(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier)]
    #[cfg_attr(all(), resilient(carrier, idempotent))]
    async fn under_a_condition_below(&self) -> Answer {
        Err(CarrierDown)
    }

    #[cfg_attr(all(), resilient(carrier))]
    #[resilient(carrier, idempotent)]
    async fn under_a_condition_above(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier)]
    #[tracing::instrument(skip_all)]
    #[shielded(carrier, idempotent)]
    async fn under_another_name_below_an_instrument(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, cancel)]
    #[shielded(carrier, idempotent)]
    async fn under_another_name_below_one_refused(&self) -> Answer {
        if token.is_cancelled() {
            return Err(CarrierDown);
        }
        Ok(())
    }
}

fn main() {}
