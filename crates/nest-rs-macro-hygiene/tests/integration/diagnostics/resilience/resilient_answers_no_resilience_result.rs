//! What a decorated method answers is read by its type: anything but a
//! `Result<T, ResilienceError<E>>` is one error at its return type, saying its
//! body answers `Result<T, E>`, whatever keys the method carries; the alias
//! above it is read as the literal is. An answer a `macro_rules!` opens on its
//! own `$crate` is told twice: that token keeps the context it resolves by,
//! so the answer spans two.

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
    async fn aliased(&self) -> Answer<String> {
        Ok(String::new())
    }

    #[resilient(carrier, idempotent)]
    async fn own_error(&self, parcel: &str) -> Result<String, CarrierDown> {
        if parcel.is_empty() {
            return Err(CarrierDown);
        }
        Ok(parcel.to_owned())
    }

    #[resilient(carrier)]
    async fn bare(&self) -> String {
        String::new()
    }

    #[resilient(carrier)]
    async fn nothing(&self) {}

    async fn stale(&self, _error: ResilienceError<CarrierDown>) -> Answer<String> {
        Ok(String::new())
    }

    #[resilient(carrier, record_if = |_| true, fallback = stale, cancel = token)]
    async fn keyed(&self) -> Result<String, CarrierDown> {
        drop(token);
        Ok(String::new())
    }
}

pub struct Unread;

macro_rules! declare_crate_rooted {
    ($name:ident) => {
        impl ShipmentsService {
            #[resilient(carrier)]
            async fn $name(&self) -> $crate::Unread {
                $crate::Unread
            }
        }
    };
}

declare_crate_rooted!(rooted);

fn main() {}
