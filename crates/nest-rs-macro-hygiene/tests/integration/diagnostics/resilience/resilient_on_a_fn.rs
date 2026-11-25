//! The policy's run is awaited, so `#[resilient]` decorates an `async fn`: a
//! `fn` is refused saying to write one, and nothing else is said of it. One
//! whose body awaits is kept as the `async fn` it was meant as, so neither its
//! `.await` nor a caller's is an error of rustc's; one that does not is kept
//! with no body, where an `Err(e)` written as an attempt's would be one.

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
    #[resilient(carrier)]
    fn ship(&self) -> Answer {
        Ok(())
    }

    #[resilient(carrier, idempotent)]
    fn track(&self, parcel: &str) -> Answer<usize> {
        let found = locate(parcel).await?;
        Ok(found)
    }

    #[resilient(carrier)]
    fn refuse(&self, parcel: &str) -> Answer<usize> {
        if parcel.is_empty() {
            return Err(CarrierDown);
        }
        Ok(parcel.len())
    }

    async fn tracked(&self) -> Answer<usize> {
        self.track("parcel").await
    }
}

async fn locate(parcel: &str) -> Result<usize, CarrierDown> {
    Ok(parcel.len())
}

fn main() {}
