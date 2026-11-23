//! An idempotent method's attempts each borrow the arguments it was called
//! with, so its future holds a shared borrow of each across its waits: it is
//! `Send` only where every argument it takes by value is `Sync`. A `Cell` is
//! rustc's `E0277` where the future is sent, pointing at `idempotent`, where
//! the replay is declared. Run once, the method moves its arguments into its
//! one attempt, and its future is `Send` as the undecorated method's is.

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
    #[resilient(carrier)]
    async fn ship(&self, tally: Cell<u32>) -> Result<u32, ResilienceError<CarrierDown>> {
        Ok(tally.get())
    }

    #[resilient(carrier, idempotent)]
    async fn track(&self, tally: Cell<u32>) -> Result<u32, ResilienceError<CarrierDown>> {
        Ok(tally.get())
    }
}

fn sent<F: Future + Send>(_: F) {}

fn main() {
    let parcels: Option<&Parcels> = None;
    if let Some(parcels) = parcels {
        sent(parcels.ship(Cell::new(1)));
        sent(parcels.track(Cell::new(1)));
    }
}
