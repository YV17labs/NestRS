//! A queue's compile-time identity in its two shapes: a static queue, which is
//! its own destination, and a dynamic queue, whose destinations are its
//! instances.

use std::any::TypeId;

use nest_rs_queue::{Destination, Queue, QueueError, QueueKind};

use crate::{SyncCommand, TenantQueue, TranscodeCommand, TranscodeQueue};

fn job_of<D: Destination>(_: &D) -> TypeId {
    TypeId::of::<D::Job>()
}

#[test]
fn a_static_queue_carries_its_name_its_kind_and_is_its_own_destination() {
    assert_eq!(<TranscodeQueue as Queue>::NAME, "transcode");
    assert_eq!(<TranscodeQueue as Queue>::KIND, QueueKind::Static);
    let name = TranscodeQueue.queue_name().expect("a valid name");
    assert_eq!(name.as_str(), "transcode");
    assert_eq!(name.kind(), QueueKind::Static);
    assert_eq!(job_of(&TranscodeQueue), TypeId::of::<TranscodeCommand>());
}

#[test]
fn a_dynamic_queue_names_one_instance_per_key() {
    assert_eq!(<TenantQueue as Queue>::NAME, "tenant");
    assert_eq!(<TenantQueue as Queue>::KIND, QueueKind::Dynamic);

    let acme = TenantQueue::instance("acme").expect("a valid key");
    let name = acme.queue_name().expect("an instance has a name");
    assert_eq!(name.as_str(), "tenant#acme");
    assert_eq!(name.queue(), "tenant");
    assert_eq!(name.instance_key(), Some("acme"));
    assert_eq!(job_of(&acme), TypeId::of::<SyncCommand>());
}

#[test]
fn an_instance_key_outside_the_rule_is_refused_when_the_instance_is_made() {
    for key in ["", "a:b", "a#b", "with space", "tab\tkey"] {
        let refused = TenantQueue::instance(key).expect_err("refused");
        assert!(
            matches!(
                refused,
                QueueError::InvalidQueueName {
                    what: "dynamic queue key",
                    ..
                }
            ),
            "{key:?}: {refused}",
        );
    }
}
