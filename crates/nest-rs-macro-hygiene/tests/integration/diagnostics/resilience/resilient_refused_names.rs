//! `fallback` names a method of `Self` and `cancel` the attempt's token, each
//! by a name written alone: a path, a call, a literal, a key written bare and a
//! method naming itself are each refused where they are written. A body
//! reading a token the decorator read no name for adds no error, and a token
//! named and never read is the developer's unused variable, at the name.

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
    async fn stale(&self, _error: ResilienceError<CarrierDown>) -> Answer {
        Ok(())
    }

    #[resilient(carrier, fallback = self.stale)]
    async fn through_self(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = Self::stale)]
    async fn by_path(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = |_error| async { Ok(()) })]
    async fn by_closure(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback)]
    async fn bare_fallback(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, fallback = itself)]
    async fn itself(&self, _error: ResilienceError<CarrierDown>) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, cancel = "token")]
    async fn by_literal(&self) -> Answer {
        if token.is_cancelled() {
            return Err(CarrierDown);
        }
        Ok(())
    }

    #[resilient(carrier, cancel)]
    async fn bare_cancel(&self) -> Answer {
        if token.is_cancelled() {
            return Err(CarrierDown);
        }
        Ok(())
    }

    #[resilient(carrier, idempotent, record_if, cancel = token)]
    async fn bare_predicate(&self) -> Answer {
        if token.is_cancelled() {
            return Err(CarrierDown);
        }
        Ok(())
    }

    #[resilient(carrier, cancel = token)]
    async fn unread_once(&self) -> Answer {
        Err(CarrierDown)
    }

    #[resilient(carrier, idempotent, cancel = token)]
    async fn unread_again(&self) -> Answer {
        Err(CarrierDown)
    }
}

fn main() {}
