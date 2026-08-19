//! The queue member of the missing-required-key family. `#[queue]` has two
//! required keys and this pins the first; the sentence is shared, so the second
//! reads the same.

use nest_rs_queue::queue;

#[queue(job = Payload)]
pub struct Emails;

#[derive(Clone)]
pub struct Payload;

fn main() {}
