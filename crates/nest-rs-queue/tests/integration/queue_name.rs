//! The typed handle itself: a `QueueName` type carries one wire name and one
//! payload type, both readable at a use site.

use nest_rs_queue::{Job, QueueName};

use crate::{TranscodeCommand, TranscodeQueue};

#[test]
fn queue_name_carries_the_wire_name_and_payload_type() {
    assert_eq!(<TranscodeQueue as QueueName>::NAME, "transcode");
    // `Self::Job` is the payload type — assert it via a job round-trip.
    fn round_trip<Q: QueueName>(job: Q::Job) -> Q::Job
    where
        Q::Job: Job,
    {
        job
    }
    let job = round_trip::<TranscodeQueue>(TranscodeCommand {
        file: "a.wav".into(),
    });
    assert_eq!(job.file, "a.wav");
}
