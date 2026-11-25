//! The expansion writes the method's return type where `impl Trait` names no
//! type: refused at the `impl Trait`, saying to name the type or box it. Only
//! the body names that type, so the method keeps it whatever else is refused:
//! a token named as an argument adds its refusal and no error of a stand-in,
//! and a token the decorator read a name for is bound for the body reading it.
//! Where the `impl Trait` sits inside the answer, the body is checked as an
//! attempt of it: an `Err(e)` and a `?` written as the decorator has them
//! written add no error of rustc's.

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
    #[resilient(carrier)]
    async fn label(&self) -> Result<impl std::fmt::Display, ResilienceError<CarrierDown>> {
        Ok("labelled")
    }

    #[resilient(carrier, idempotent, cancel = stop)]
    async fn label_or_fail(
        &self,
        parcel: String,
    ) -> Result<impl std::fmt::Display, ResilienceError<CarrierDown>> {
        if stop.is_cancelled() {
            return Err(CarrierDown);
        }
        let weighed: Result<usize, CarrierDown> = Ok(parcel.len());
        Ok(weighed?)
    }

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
