//! Integration coverage for the **typed queue handle** surface: a `QueueName`
//! type links a producer's `push_to::<Q>` and a consumer's
//! `#[process(queue = Q)]` to one wire name and one payload type. The round-trip
//! here stays in-process — a fake `JobProducer` records pushes, and the
//! `#[process]`-emitted handler is drained straight from the link-time inventory
//! and invoked with an envelope payload. Live-Redis coverage is the worker
//! app's e2e suite.
//!
//! **This suite is also the call-site hygiene proof for `#[processor]`.** The
//! manifest declares no `nest-rs-worker`, so a `#[process]` expansion that
//! names it directly (it used to emit `::nest_rs_worker::run_in_job_context`)
//! fails to compile *here* — which is exactly what a freshly generated
//! `crates/features` sees, since nothing tells a developer writing a processor
//! that the worker crate is a hard requirement.
//!
//! Suite root: each test lives in the module named for the `src/` concern it
//! covers ([`queue_name`], [`producer`], [`processor`]); this file holds only
//! the fixture both sides share — the queue's single identity artifact.

mod diagnostics;
mod processor;
mod producer;
mod queue_name;

use nest_rs_queue::queue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TranscodeCommand {
    file: String,
}

// The single artifact both sides import: name + payload type in one place.
#[queue(name = "transcode", job = TranscodeCommand)]
struct TranscodeQueue;
