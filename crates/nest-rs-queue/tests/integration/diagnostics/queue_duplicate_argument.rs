//! A `#[queue]` key written twice is refused rather than resolved by source
//! order.
//!
//! The dropped declaration here is the queue *name* — the one string a producer
//! and its consumer have to agree on, and the one whose disagreement shows up as
//! jobs nobody consumes rather than as an error.

use nest_rs_queue::queue;
use nest_rs_core::input;

#[input]
#[derive(Clone)]
struct TranscodeCommand {
    file: String,
}

#[queue(name = "transcode", name = "transcode-v2", job = TranscodeCommand)]
struct TranscodeQueue;

fn main() {}
