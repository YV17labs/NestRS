//! Run once under `record_if`, the one attempt borrows each argument it only
//! reads, which the predicate may read once it answered, so the method's
//! future holds a shared borrow of it across its waits: it is `Send` only
//! where such an argument is `Sync`. A `Cell` is rustc's own error where the
//! future is sent, naming the type and the wait at the method's answer, no
//! key. An argument the body consumes is the attempt's own, and asks nothing.

use std::cell::Cell;
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

#[injectable]
struct Parcels {
    #[inject]
    carrier: Arc<ResiliencePolicy>,
}

impl Parcels {
    #[resilient(carrier, record_if = |_error| true)]
    async fn ship(&self, tally: Cell<u32>) -> Result<u32, ResilienceError<CarrierDown>> {
        Ok(tally.get())
    }

    #[resilient(carrier, record_if = |_error| true)]
    async fn weigh(&self, tally: Cell<u32>) -> Result<u32, ResilienceError<CarrierDown>> {
        Ok(tally.into_inner())
    }
}

fn sent<F: Future + Send>(_: F) {}

fn main() {
    let parcels: Option<&Parcels> = None;
    if let Some(parcels) = parcels {
        sent(parcels.ship(Cell::new(1)));
        sent(parcels.weigh(Cell::new(1)));
    }
}
