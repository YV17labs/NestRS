//! A queue's compile-time identity: its wire name, its payload, and the marker
//! that is its own destination.

use std::any::TypeId;

use nest_rs_queue::{Destination, Queue};

use crate::{TranscodeCommand, TranscodeQueue};

fn job_of<D: Destination>(_: &D) -> TypeId {
    TypeId::of::<D::Job>()
}

#[test]
fn a_queue_carries_its_name_and_is_its_own_destination() {
    assert_eq!(<TranscodeQueue as Queue>::NAME, "transcode");
    let name = TranscodeQueue.queue_name().expect("a valid name");
    assert_eq!(name.as_str(), "transcode");
    assert_eq!(job_of(&TranscodeQueue), TypeId::of::<TranscodeCommand>());
}
