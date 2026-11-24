//! Under `fallback` the fallback is handed the arguments the method was called
//! with, each under the name it is bound to: a `mut` binding, a `&mut`
//! parameter and a parameter bound by a pattern are each refused at what was
//! written, and the body, which consumes its argument, adds no error.

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

async fn send(order: String) -> Result<usize, CarrierDown> {
    Ok(order.len())
}

impl ShipmentsService {
    async fn queued(&self, order: String, _error: ResilienceError<CarrierDown>) -> Answer<usize> {
        Ok(order.len())
    }

    #[resilient(carrier, fallback = queued)]
    async fn label(&self, mut order: String) -> Answer<usize> {
        order.push_str(" (labelled)");
        send(order).await
    }

    #[resilient(carrier, fallback = queued)]
    async fn stamp(&self, order: &mut String) -> Answer<usize> {
        order.push_str(" (stamped)");
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = queued)]
    async fn pair(&self, (order, _zone): (String, u8)) -> Answer<usize> {
        send(order).await
    }

    #[resilient(carrier, fallback = queued)]
    async fn unnamed(&self, _: String) -> Answer<usize> {
        Err(CarrierDown)
    }
}

fn main() {}
