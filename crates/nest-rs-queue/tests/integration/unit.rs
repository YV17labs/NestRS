//! Covers `src/unit.rs` from the site's side: the declaration each `#[process]`
//! method emits, naming its unit and carrying the impl block's and its own
//! `#[meta]`.

use nest_rs_core::Posture;
use nest_rs_queue::{Queue, processor};

use crate::{SyncCommand, SyncQueue, TranscodeCommand, TranscodeQueue};

#[derive(Debug, PartialEq)]
struct Audit(&'static str);

#[derive(Debug, PartialEq)]
struct Cost(u32);

/// Declared only on a site no test reads, so built by its declaration alone.
#[deny(dead_code)]
struct Unread;

struct MeteredProcessor;

impl nest_rs_core::ProviderResidency for MeteredProcessor {
    const SINGLETON: bool = true;
}

#[processor]
#[meta(Audit("processor"))]
#[meta(Cost(1))]
impl MeteredProcessor {
    #[process(queue = TranscodeQueue)]
    #[meta(Cost(5))]
    async fn transcode(&self, _job: TranscodeCommand) -> anyhow::Result<()> {
        Ok(())
    }

    #[process(queue = SyncQueue)]
    #[meta(Unread)]
    async fn idle(&self, _job: SyncCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn a_process_method_declares_its_unit_address_and_metadata() {
    let handler = MeteredProcessor::__NESTRS_HANDLER_TRANSCODE;
    assert_eq!(handler.unit(), nest_rs_queue::unit::JOB);
    assert_eq!(handler.host_name(), "MeteredProcessor");
    assert_eq!(handler.method(), "transcode");
    assert_eq!(handler.address(), <TranscodeQueue as Queue>::NAME);
    assert_eq!(handler.posture(), Posture::System);
    let reflector = handler.reflector();
    assert_eq!(reflector.get::<Cost>(), Some(&Cost(5)));
    assert_eq!(reflector.get::<Audit>(), Some(&Audit("processor")));
}
