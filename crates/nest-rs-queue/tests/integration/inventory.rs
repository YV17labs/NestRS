//! The entry `#[processor]` submits for each `#[process]` method: the queue it
//! drains, every key it declares, and what those declarations need from a
//! backend.

use std::num::NonZeroU32;
use std::time::Duration;

use nest_rs_queue::{
    Capabilities, Capability, Checkpoint, ProcessMethod, Queue, QueueKind, Throttle, processor,
};

use crate::{SyncCommand, TenantQueue, TranscodeCommand, TranscodeQueue, method};

#[test]
fn a_process_method_is_submitted_with_its_queue_and_its_options() {
    let transcode = method("TranscodeProcessor::transcode");
    assert_eq!(transcode.queue(), <TranscodeQueue as Queue>::NAME);
    assert_eq!(transcode.queue_kind(), QueueKind::Static);
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
        transcode.required_capabilities(),
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
        queue = TenantQueue,
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
    // The literal is right here, and reading it as a copy was the mistake: the
    // fixture *declares* `prefix = "tenant"` three lines up, so `"tenant"` is
    // this test's own input, while `<TenantQueue as Queue>::NAME` is the very
    // const `#[processor]` emits — asserting one against the other can only fail
    // if the macro stops emitting it, never if it emits the wrong name. Proved:
    // mangling `#[queue]`'s emitted `NAME` keeps this assertion green.
    assert_eq!(sync.queue(), "tenant");
    assert_eq!(sync.queue_kind(), QueueKind::Dynamic);
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

    let required = sync.required_capabilities();
    for capability in [
        Capability::Throttle,
        Capability::Checkpoint,
        Capability::DynamicQueues,
    ] {
        assert!(required.contains(capability), "{capability:?}");
    }
    assert!(
        !required.contains(Capability::DelayedPush),
        "a push option is not a method's to need",
    );
}

/// Keys read through a `macro_rules!` fragment.
///
/// `syn` wraps a `$value:expr` substitution in an invisible-delimiter group, and
/// whether it unwraps depends on how the argument list was parsed — so the same
/// value was once accepted at one site and refused at another. The job
/// decorators answer identically or a shared key is a fiction.
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
    // Compiling is most of the assertion; this pins that both methods reached
    // the inventory, and that a number forwarded through a fragment is the
    // number written.
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
    #[allow(clippy::needless_arbitrary_self_type)]
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

#[allow(non_camel_case_types)]
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

/// Compiling is the first half: a method compiled out — by a `#[cfg]`, or a
/// `#[cfg]` inside a `#[cfg_attr]`, whatever its predicate — takes its handler,
/// its payload check and its entry with it; without them the expansion names a
/// method, a queue and a job that do not exist. The typed `self: &Self` is `&self`
/// spelled out, `self: &Arc<Self>` borrows what the container holds, and a raw
/// identifier is its name — a method's, where the expansion panicked building a
/// handler from `r#`, and a host's.
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
