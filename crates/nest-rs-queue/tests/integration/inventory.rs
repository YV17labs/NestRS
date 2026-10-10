//! The entry `#[processor]` submits for each `#[process]` method: the queue it
//! drains, every key it declares, and what those declarations need from a
//! backend.

use std::num::NonZeroU32;
use std::time::Duration;

use nest_rs_queue::{
    Capabilities, Capability, Checkpoint, ProcessMethod, Queue, Throttle, processor,
};

use crate::{SyncCommand, SyncQueue, TranscodeCommand, TranscodeQueue, method};

#[test]
fn a_process_method_is_submitted_with_its_queue_and_its_options() {
    let transcode = method("TranscodeProcessor::transcode");
    assert_eq!(transcode.queue(), <TranscodeQueue as Queue>::NAME);
    let options = transcode.options();
    assert_eq!(options.retries(), 1);
    assert_eq!(
        options.concurrency().get(),
        1,
        "one job at a time unless the method says otherwise",
    );
    assert_eq!(options.throttle(), None);
    assert!(!options.checkpoint());
    assert_eq!(
        nest_rs_queue::__private::required_capabilities(transcode),
        Capabilities::NONE,
        "a plain method needs nothing a backend may lack",
    );
}

struct SyncProcessor;

impl nest_rs_core::ProviderResidency for SyncProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl SyncProcessor {
    #[process(
        queue = SyncQueue,
        retries = 2,
        concurrency = 4,
        throttle(limit = 10, window = "1m"),
        transactional = false
    )]
    async fn sync(&self, job: SyncCommand, checkpoint: Checkpoint<u32>) -> anyhow::Result<()> {
        let _ = (job, checkpoint.get());
        Ok(())
    }
}

#[test]
fn every_key_reaches_the_options_and_the_capabilities_it_needs() {
    let sync = method("SyncProcessor::sync");
    // A literal on purpose: `<SyncQueue as Queue>::NAME` is the const the macro
    // emits, and asserting against it would pass a wrong name.
    assert_eq!(sync.queue(), "sync");
    let options = sync.options();
    assert_eq!(options.retries(), 2);
    assert_eq!(options.concurrency(), NonZeroU32::new(4).expect("non-zero"));
    assert_eq!(
        options.throttle(),
        Some(Throttle::new(
            NonZeroU32::new(10).expect("non-zero"),
            Duration::from_secs(60),
        )),
    );
    assert!(
        options.checkpoint(),
        "a `Checkpoint<_>` parameter is a declaration"
    );

    let required = nest_rs_queue::__private::required_capabilities(sync);
    for capability in [Capability::Throttle, Capability::Checkpoint] {
        assert!(required.contains(capability), "{capability:?}");
    }
    assert!(
        !required.contains(Capability::DelayedPush),
        "a push option is not a method's to need",
    );
}

/// Keys read through a `macro_rules!` fragment: `syn` wraps a `$value:expr`
/// substitution in an invisible-delimiter group, unwrapped by one parse and not another.
macro_rules! declare_fragment_jobs {
    ($settle:expr, $slots:expr) => {
        struct FragmentProcessor;

        impl nest_rs_core::ProviderResidency for FragmentProcessor {
            const SINGLETON: bool = true;
        }

        #[processor]
        impl FragmentProcessor {
            #[process(queue = TranscodeQueue, transactional = $settle)]
            async fn first(&self, _job: TranscodeCommand) -> anyhow::Result<()> {
                Ok(())
            }

            #[process(queue = TranscodeQueue, transactional = $settle, concurrency = $slots)]
            async fn second(&self, _job: TranscodeCommand) -> anyhow::Result<()> {
                Ok(())
            }
        }
    };
}

declare_fragment_jobs!(false, 3);

#[test]
fn a_fragment_is_read_the_same_wherever_the_key_sits() {
    let entries: Vec<&ProcessMethod> = nest_rs_core::inventory::iter::<ProcessMethod>()
        .filter(|method| method.name().starts_with("FragmentProcessor::"))
        .collect();
    assert_eq!(
        entries.len(),
        2,
        "both fragment-declared methods registered"
    );
    assert_eq!(
        method("FragmentProcessor::second")
            .options()
            .concurrency()
            .get(),
        3,
    );
}

#[cfg(any())]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CompiledOutCommand;

#[cfg(any())]
#[nest_rs_queue::queue(name = "compiled-out", job = CompiledOutCommand)]
struct CompiledOutQueue;

struct ShapedProcessor;

impl nest_rs_core::ProviderResidency for ShapedProcessor {
    const SINGLETON: bool = true;
}

#[processor]
impl ShapedProcessor {
    #[expect(
        clippy::needless_arbitrary_self_type,
        reason = "the spelled-out receiver is the shape under test"
    )]
    #[process(queue = TranscodeQueue)]
    async fn typed_receiver(self: &Self, _job: TranscodeCommand) -> anyhow::Result<()> {
        Ok(())
    }

    #[cfg(any())]
    #[process(queue = CompiledOutQueue)]
    async fn compiled_out(&self, _job: CompiledOutCommand) -> anyhow::Result<()> {
        Ok(())
    }

    #[cfg_attr(all(), cfg(any()))]
    #[process(queue = CompiledOutQueue)]
    async fn compiled_out_by_cfg_attr(&self, _job: CompiledOutCommand) -> anyhow::Result<()> {
        Ok(())
    }

    #[process(queue = TranscodeQueue)]
    async fn r#type(&self, _job: TranscodeCommand) -> anyhow::Result<()> {
        Ok(())
    }

    #[process(queue = TranscodeQueue)]
    async fn arc_receiver(
        self: &std::sync::Arc<Self>,
        _job: TranscodeCommand,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    #[cfg_attr(true, cfg(false))]
    #[process(queue = CompiledOutQueue)]
    async fn compiled_out_by_a_boolean_predicate(
        &self,
        _job: CompiledOutCommand,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[expect(
    non_camel_case_types,
    reason = "a raw-identifier type is the shape under test"
)]
struct r#yield;

impl nest_rs_core::ProviderResidency for r#yield {
    const SINGLETON: bool = true;
}

#[processor]
impl r#yield {
    #[process(queue = TranscodeQueue)]
    async fn run(&self, _job: TranscodeCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn a_compiled_out_method_submits_nothing_and_a_typed_receiver_is_served() {
    let mut names: Vec<&str> = nest_rs_core::inventory::iter::<ProcessMethod>()
        .map(ProcessMethod::name)
        .filter(|name| name.starts_with("ShapedProcessor::") || name.starts_with("yield::"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "ShapedProcessor::arc_receiver",
            "ShapedProcessor::type",
            "ShapedProcessor::typed_receiver",
            "yield::run",
        ]
    );
}
