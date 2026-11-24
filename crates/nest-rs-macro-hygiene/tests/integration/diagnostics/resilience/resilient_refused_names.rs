//! `fallback` names a method of `Self` and `cancel` the attempt's token, each
//! by a name alone: a path, a call, a literal, a bare key, a method naming
//! itself and a token named as an argument are each refused where written. A
//! body reading a token the decorator bound under no name adds no error, and a
//! token named and never read is the developer's unused variable, at the name.

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

    #[resilient(carrier, cancel = token)]
    async fn named_as_an_argument(&self, token: String) -> Answer {
        if token.is_empty() {
            return Err(CarrierDown);
        }
        Ok(())
    }

    async fn unverified(&self, _token: String, _error: ResilienceError<CarrierDown>) -> Answer {
        Ok(())
    }

    #[resilient(carrier, fallback = unverified, cancel = token)]
    async fn named_as_what_its_fallback_takes(&self, token: String) -> Answer {
        drop(format!("{token:?}"));
        Ok(())
    }
}

fn main() {}
