//! A setting the policy holds for every caller, written where the field
//! belongs — with a value, before the field, or alone — refused at the key in
//! the words it earns after the field: which mechanism shares it and where it
//! is set, rather than as a field the list names wrong. Each body answers
//! through a coercion the undecorated method makes — a `String` read as a
//! `str`, a box unsized at an early `return` and at its tail — and none adds
//! an error of its own: a refused method's refusals stand in its body's place.

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

/// The labels a parcel carries, answered boxed.
type Labels = Box<dyn Iterator<Item = &'static str> + Send>;

#[injectable]
struct ShipmentsService {
    #[inject]
    carrier: Arc<ResiliencePolicy>,
    hub: String,
}

impl ShipmentsService {
    #[resilient(rate_limit = 5)]
    async fn limited<'a>(&'a self) -> Answer<&'a str> {
        Ok(&self.hub)
    }

    #[resilient(breaker_failures = 5, carrier)]
    async fn tripped(&self, parcel: &str) -> Answer<Labels> {
        if parcel.is_empty() {
            return Ok(Box::new(std::iter::empty()));
        }
        Ok(Box::new(["fragile"].into_iter()))
    }

    #[resilient(concurrency)]
    async fn bounded(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
