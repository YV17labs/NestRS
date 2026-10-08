//! The queue port in process: its decorators, pushes and attempts, against a
//! backend declaring every capability and one declaring none.
//!
//! The manifest declares no `nest-rs-worker`: this suite is the call-site
//! hygiene proof for `#[processor]`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod checkpoint;
mod consume;
mod inventory;
#[path = "../harness/memory.rs"]
mod memory;
mod producer;
mod queue;
mod queue_name;
mod worker;

use std::any::TypeId;
use std::sync::Mutex;

use nest_rs_core::{Container, ReachableProviders};
use nest_rs_queue::{Capabilities, Capability, ProcessMethod, QueueBackend, processor, queue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TranscodeCommand {
    file: String,
}

#[queue(name = "transcode", job = TranscodeCommand)]
struct TranscodeQueue;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SyncCommand {
    org: String,
}

#[queue(name = "sync", job = SyncCommand)]
struct SyncQueue;

/// A backend declaring no optional capability — what every refusal is proved
/// against.
static BARE: QueueBackend = QueueBackend::new("bare", Capabilities::NONE);

/// A backend declaring every optional capability, one by one — the way a real
/// backend declares what it honours.
static FULL: QueueBackend = QueueBackend::new(
    "full",
    Capabilities::NONE
        .with(Capability::DelayedPush)
        .with(Capability::UniquePush)
        .with(Capability::Cancellation)
        .with(Capability::Throttle)
        .with(Capability::Checkpoint),
);

/// The files `TranscodeProcessor` saw. Process-wide, and nextest runs each test
/// in a process of its own.
static TRANSCODED: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct TranscodeProcessor;

// Hand-provided: no decorator built this type to state its residency.
impl nest_rs_core::ProviderResidency for TranscodeProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl TranscodeProcessor {
    #[process(queue = TranscodeQueue, retries = 1)]
    async fn transcode(&self, job: TranscodeCommand) -> anyhow::Result<()> {
        TRANSCODED.lock().expect("lock").push(job.file);
        Ok(())
    }
}

/// `answer`, or a panic once twice the port's net has passed — so a net lost
/// fails its test instead of hanging it.
async fn within_twice_the_net<T>(answer: impl std::future::Future<Output = T>) -> T {
    let deadline = nest_rs_queue::BACKEND_TIMEOUT * 2;
    tokio::time::timeout(deadline, answer)
        .await
        .unwrap_or_else(|_| panic!("no answer within twice the port's net ({deadline:?})"))
}

/// The inventory entry `#[processor]` submitted for `name`.
fn method(name: &str) -> &'static ProcessMethod {
    nest_rs_core::inventory::iter::<ProcessMethod>()
        .find(|method| method.name() == name)
        .unwrap_or_else(|| panic!("`{name}` is submitted to the inventory"))
}

/// A container reaching exactly `providers` — the gate `consume::discover`
/// reads, so a test sees only the entries it names among every entry linked
/// into this binary.
fn reaching(providers: &[TypeId]) -> Container {
    Container::builder()
        .provide(ReachableProviders(providers.iter().copied().collect()))
        .build()
}
