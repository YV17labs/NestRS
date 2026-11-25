//! A predicate key takes a `Fn` over a borrow of what it judges — the method's
//! error, what it answers, or the error its run ended on — answering `bool`:
//! a value of another shape is one error, at the value, in the key's sentence.
//! So is a function of another arity: a method taking `&self` beside what it
//! judges, which a closure calls, and a function taking nothing.

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

struct Quote;

impl Quote {
    fn pending(&self) -> bool {
        false
    }
}

impl CarrierDown {
    fn transient(&self) -> bool {
        true
    }
}

impl ShipmentsService {
    async fn stale(&self, _error: ResilienceError<CarrierDown>) -> Answer<Quote> {
        Ok(Quote)
    }

    #[resilient(carrier, idempotent, retry_if = Quote::pending)]
    async fn retried(&self) -> Answer<Quote> {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, retry_ok_if = CarrierDown::transient)]
    async fn polled(&self) -> Answer<Quote> {
        Err(CarrierDown)
    }

    #[resilient(carrier, record_if = |error: CarrierDown| error.transient())]
    async fn recorded(&self) -> Answer<Quote> {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = stale, fallback_if = CarrierDown::transient)]
    async fn filtered(&self) -> Answer<Quote> {
        Err(CarrierDown)
    }

    fn sent_again(&self, _error: &CarrierDown) -> bool {
        true
    }

    fn pending(&self, _quote: &Quote) -> bool {
        false
    }

    fn served(&self, _error: &ResilienceError<CarrierDown>) -> bool {
        true
    }

    #[resilient(carrier, idempotent, retry_if = Self::sent_again)]
    async fn retried_by_a_method(&self) -> Answer<Quote> {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, retry_ok_if = Self::pending)]
    async fn polled_by_a_method(&self) -> Answer<Quote> {
        Err(CarrierDown)
    }

    #[resilient(carrier, record_if = Self::sent_again)]
    async fn recorded_by_a_method(&self) -> Answer<Quote> {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = stale, fallback_if = Self::served)]
    async fn filtered_by_a_method(&self) -> Answer<Quote> {
        Err(CarrierDown)
    }

    #[resilient(carrier, record_if = always)]
    async fn recorded_by_a_function_of_nothing(&self) -> Answer<Quote> {
        Err(CarrierDown)
    }
}

fn always() -> bool {
    true
}

fn main() {}
