//! Covers `src/way_down.rs` — a signal received while the process is already
//! stopping. A process exiting at once cannot be asserted on from inside, so the
//! parent signals a [`ChildProcess`].

use std::time::Duration;

use nest_rs_core::__private::TransportContribution;
use nest_rs_core::{
    App, Container, ContainerBuilder, Module, Registering, Transport, hooks, injectable, module,
};
use tokio_util::sync::CancellationToken;

use crate::ChildProcess;

const CHILD_TEST: &str = "way_down::way_down_child_process";

const READY: &str = "WAY-DOWN-CHILD SERVING";

const HOOK_STARTED: &str = "WAY-DOWN-CHILD HOOK STARTED";

/// The handlers are installed before any transport serves, so a parent reading
/// [`READY`] may signal.
struct ServesUntilStopped;

#[async_trait::async_trait]
impl Transport for ServesUntilStopped {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        println!("{READY}");
        cancel.cancelled().await;
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct NeverStops;

#[async_trait::async_trait]
impl Transport for NeverStops {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    async fn serve(self: Box<Self>, _cancel: CancellationToken) -> anyhow::Result<()> {
        println!("{READY}");
        std::future::pending::<()>().await;
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::MAX
    }
}

struct NeverStopsModule;

impl Module for NeverStopsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "NeverStops",
            build: |_| Ok(Box::new(NeverStops)),
        })
    }
}

struct ServesUntilStoppedModule;

impl Module for ServesUntilStoppedModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "ServesUntilStopped",
            build: |_| Ok(Box::new(ServesUntilStopped)),
        })
    }
}

#[injectable]
#[derive(Default)]
struct HangsOnDestroy;

#[hooks]
impl HangsOnDestroy {
    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    #[on_module_destroy]
    async fn release(&self) {
        println!("{HOOK_STARTED}");
        std::future::pending::<()>().await;
    }
}

#[module(providers = [HangsOnDestroy])]
struct HangsOnDestroyModule;

#[nest_rs_core::main]
async fn run_child(role: String) -> anyhow::Result<()> {
    match role.as_str() {
        "hook" => {
            App::builder()
                .module::<ServesUntilStoppedModule>()
                .module::<HangsOnDestroyModule>()
                .build()
                .await?
                .run()
                .await
        }
        "transport" => App::new::<NeverStopsModule>()?.run().await,
        other => anyhow::bail!("no child role `{other}`"),
    }
}

#[test]
fn way_down_child_process() {
    if let Some(role) = crate::child_role() {
        let _ = run_child(role);
    }
}

const EXITING: &str =
    "shutdown signal received on the way down: exiting at once, abandoning what still runs";

#[cfg(unix)]
#[test]
fn a_second_signal_while_a_hook_hangs_exits_at_once_naming_the_hook() {
    let mut child = ChildProcess::spawn(CHILD_TEST, "hook");
    child.expect_line(READY);
    child.signal("TERM");
    child.expect_line(HOOK_STARTED);
    child.signal("INT");

    let (code, printed) = child.exit_within(Duration::from_secs(2));

    assert_eq!(code, Some(130), "SIGINT's code: {printed:#?}");
    let line = printed
        .iter()
        .find(|line| line.contains(EXITING))
        .unwrap_or_else(|| panic!("the line naming what it abandons: {printed:#?}"));
    assert!(line.contains("HangsOnDestroy"), "{line}");
    assert!(line.contains("release"), "{line}");
    assert!(line.contains("SIGINT"), "{line}");
}

#[cfg(unix)]
#[test]
fn a_second_signal_while_a_transport_will_not_stop_exits_at_once_naming_it() {
    let mut child = ChildProcess::spawn(CHILD_TEST, "transport");
    child.expect_line(READY);
    child.signal("TERM");
    child.expect_line("shutdown signal received");
    child.signal("TERM");

    let (code, printed) = child.exit_within(Duration::from_secs(2));

    assert_eq!(code, Some(143), "SIGTERM's code: {printed:#?}");
    let line = printed
        .iter()
        .find(|line| line.contains(EXITING))
        .unwrap_or_else(|| panic!("the line naming what it abandons: {printed:#?}"));
    assert!(line.contains("NeverStops"), "{line}");
    assert!(line.contains("SIGTERM"), "{line}");
}
