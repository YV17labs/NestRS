//! `idempotent` attempts the body again with the arguments the method was
//! called with, so a `mut` binding or a `&mut` parameter would carry one
//! attempt's change into the next: each is refused at what was written, ahead
//! of rustc's `FnMut` sentence, which names nothing the developer wrote.

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
    #[resilient(carrier, idempotent)]
    async fn label(&self, mut order: String) -> Answer<String> {
        order.push_str(" (labelled)");
        Ok(order)
    }

    #[resilient(carrier, idempotent)]
    async fn stamp(&self, order: &mut String) -> Answer {
        order.push_str(" (stamped)");
        Err(CarrierDown)
    }
}

fn main() {}
