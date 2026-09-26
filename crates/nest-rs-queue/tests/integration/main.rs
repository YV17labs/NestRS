//! Integration coverage for the queue port, in process: the `#[queue]` and
//! `#[processor]` decorators against the types they implement, pushes through a
//! backend that records them, and attempts at a job through the port's own
//! `consume` — against a backend declaring every capability and one declaring
//! none, so each refusal a capability-less backend owes is proved here, while the
//! behaviour a real backend adds is proved by that backend's e2e suite.
//!
//! **This suite is also the call-site hygiene proof for `#[processor]`.** The
//! manifest declares no `nest-rs-worker`, so a `#[process]` expansion naming it
//! directly fails to compile here — which is what a freshly generated
//! `crates/features` would see.
//!
//! Suite root: each test lives in the module named for the `src/` concern it
//! covers; this file holds the fixtures several of them share.

mod checkpoint;
mod consume;
mod diagnostics;
mod inventory;
mod producer;
mod queue;
mod queue_name;

use std::any::TypeId;
use std::sync::Mutex;

use nest_rs_core::{Container, ReachableProviders};
use nest_rs_queue::{Capabilities, Capability, ProcessMethod, QueueBackend, processor, queue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TranscodeCommand {
    file: String,
}

// A static queue: the one artifact producer and consumer both import.
#[queue(name = "transcode", job = TranscodeCommand)]
struct TranscodeQueue;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SyncCommand {
    org: String,
}

// A dynamic queue: one instance per runtime key.
#[queue(prefix = "tenant", job = SyncCommand)]
struct TenantQueue;

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
        .with(Capability::Checkpoint)
        .with(Capability::DynamicQueues),
);

/// The files `TranscodeProcessor` saw. Process-wide, and nextest runs each test
/// in a process of its own.
static TRANSCODED: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct TranscodeProcessor;

// Hand-provided below, which is a singleton registration. No decorator built
// this type, so nothing has stated its residency and the hand-written path is
// open — the one place it still is.
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
