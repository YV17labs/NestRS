//! An answer that is an `impl Trait` whole names neither what an attempt
//! answers nor its error: refused at the `impl Trait`, saying to put it inside
//! the `Result<T, ResilienceError<E>>` or name the type. The body is the
//! refusal's place, so a token named as an argument adds its own refusal and
//! nothing else, and a body reading the token it names adds no error.

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

trait Quoted {}

impl Quoted for String {}

impl ShipmentsService {
    #[resilient(carrier, cancel = token)]
    async fn named_as_an_argument(&self, token: String) -> impl Quoted {
        token
    }

    #[resilient(carrier, cancel = stop)]
    async fn read_under_its_name(&self) -> impl Quoted {
        if stop.is_cancelled() {
            return String::new();
        }
        String::from("quoted")
    }
}

fn main() {}
