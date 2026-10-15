//! Covers `src/way_down.rs` — a signal received before the app exists, during
//! its boot, and while the process is already stopping. A process exiting at
//! once cannot be asserted on from inside, so the parent signals a
//! [`ChildProcess`].

use std::time::Duration;

use nest_rs_core::__private::TransportContribution;
use nest_rs_core::{
    App, BootInterruptedError, Container, ContainerBuilder, Module, Registering, Transport, hooks,
    injectable, module,
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

/// A host whose name another module's host shares, so only its origin tells
/// them apart.
mod hangs {
    use nest_rs_core::{hooks, injectable, module};

    #[injectable]
    #[derive(Default)]
    pub(crate) struct Cache;

    #[hooks]
    impl Cache {
        #[expect(
            clippy::print_stdout,
            reason = "the child's stdout is the protocol its parent test reads"
        )]
        #[on_module_destroy]
        async fn release(&self) {
            println!("{}", super::HOOK_STARTED);
            std::future::pending::<()>().await;
        }
    }

    #[module(providers = [Cache])]
    pub(crate) struct HangsOnDestroyModule;
}

mod returns {
    use nest_rs_core::{hooks, injectable, module};

    #[injectable]
    #[derive(Default)]
    pub(crate) struct Cache;

    #[hooks]
    impl Cache {
        #[on_module_destroy]
        async fn release(&self) {}
    }

    #[module(providers = [Cache])]
    pub(crate) struct ReturnsOnDestroyModule;
}

/// Long past every bound a test waits: a step still waiting then was never
/// stopped.
const WAITS_FOR: Duration = Duration::from_secs(30);

/// What `main` printed of the error the boot returned.
const BOOT_FAILED: &str = "WAY-DOWN-CHILD BOOT FAILED";

struct SlowPool;

/// Prints [`READY`] once its `configure` is under way, then waits.
struct ConfiguresSlowly;

#[async_trait::async_trait]
impl Transport for ConfiguresSlowly {
    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        println!("{READY}");
        tokio::time::sleep(WAITS_FOR).await;
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        cancel.cancelled().await;
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        Duration::ZERO
    }
}

struct ConfiguresSlowlyModule;

impl Module for ConfiguresSlowlyModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "ConfiguresSlowly",
            build: |_| Ok(Box::new(ConfiguresSlowly)),
        })
    }
}

const FIRST_INIT_RAN: &str = "WAY-DOWN-CHILD FIRST INIT RAN";
const FIRST_TORN_DOWN: &str = "WAY-DOWN-CHILD FIRST TORN DOWN";
const WARMING_TORN_DOWN: &str = "WAY-DOWN-CHILD WARMING TORN DOWN";

#[injectable]
#[derive(Default)]
struct InitsFirst;

#[hooks]
impl InitsFirst {
    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    #[on_module_init]
    async fn register(&self) {
        println!("{FIRST_INIT_RAN}");
    }

    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    #[on_module_destroy]
    async fn deregister(&self) {
        println!("{FIRST_TORN_DOWN}");
    }
}

#[injectable]
#[derive(Default)]
struct WarmsSlowly;

#[hooks]
impl WarmsSlowly {
    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    #[on_module_init]
    async fn warm(&self) {
        println!("{READY}");
        tokio::time::sleep(WAITS_FOR).await;
    }

    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    #[on_module_destroy]
    async fn evict(&self) {
        println!("{WARMING_TORN_DOWN}");
    }
}

#[module(providers = [InitsFirst, WarmsSlowly])]
struct WarmsSlowlyModule;

#[injectable]
#[derive(Default)]
struct BlocksOnInit;

#[hooks]
impl BlocksOnInit {
    #[expect(
        clippy::print_stdout,
        reason = "the child's stdout is the protocol its parent test reads"
    )]
    #[on_module_init]
    async fn migrate(&self) {
        println!("{READY}");
        std::thread::sleep(WAITS_FOR);
    }
}

#[module(providers = [BlocksOnInit])]
struct BlocksOnInitModule;

/// Says what the boot returned, as the parent reads it: the error's chain,
/// then the step it names when it is an interruption.
#[expect(
    clippy::print_stdout,
    reason = "the child's stdout is the protocol its parent test reads"
)]
fn said(booted: anyhow::Result<()>) -> anyhow::Result<()> {
    if let Err(error) = &booted {
        match error.downcast_ref::<BootInterruptedError>() {
            Some(interrupted) => println!(
                "{BOOT_FAILED} interrupted by {:?} during {}: {error:#}",
                interrupted.cause, interrupted.step
            ),
            None => println!("{BOOT_FAILED}: {error:#}"),
        }
    }
    booted
}

#[expect(
    clippy::print_stdout,
    reason = "the child's stdout is the protocol its parent test reads"
)]
#[nest_rs_core::main]
async fn run_child(role: String) -> anyhow::Result<()> {
    match role.as_str() {
        "main" => {
            println!("{READY}");
            tokio::time::sleep(WAITS_FOR).await;
            Ok(())
        }
        "factory" => said(
            App::builder()
                .provide_factory(|_| async {
                    println!("{READY}");
                    tokio::time::sleep(WAITS_FOR).await;
                    Ok(SlowPool)
                })
                .build()
                .await
                .map(drop),
        ),
        "blocking-factory" => said(
            App::builder()
                .provide_factory(|_| async {
                    println!("{READY}");
                    std::thread::sleep(WAITS_FOR);
                    Ok(SlowPool)
                })
                .build()
                .await
                .map(drop),
        ),
        "configure" => said(App::new::<ConfiguresSlowlyModule>()?.run().await),
        "init" => said(
            App::builder()
                .module::<WarmsSlowlyModule>()
                .build()
                .await?
                .run()
                .await,
        ),
        "blocking-init" => said(App::new::<BlocksOnInitModule>()?.run().await),
        "hook" => {
            App::builder()
                .module::<ServesUntilStoppedModule>()
                .module::<returns::ReturnsOnDestroyModule>()
                .module::<hangs::HangsOnDestroyModule>()
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
    match crate::child_role().as_deref() {
        Some("fallback") => {
            let _ = run_on_a_runtime_of_its_own();
        }
        Some(role) => {
            let _ = run_child(role.to_owned());
        }
        None => {}
    }
}

/// An app run on a runtime `#[nest_rs::main]` did not build, as a
/// `#[tokio::test]` runs it: no process watcher exists.
fn run_on_a_runtime_of_its_own() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async { App::new::<ServesUntilStoppedModule>()?.run().await })
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
    assert!(line.contains("Cache"), "{line}");
    assert!(line.contains("integration::way_down::hangs"), "{line}");
    assert!(line.contains("release"), "{line}");
    assert!(line.contains("SIGINT"), "{line}");
}

#[cfg(unix)]
#[test]
fn a_second_signal_while_a_transport_will_not_stop_exits_at_once_naming_it() {
    let mut child = ChildProcess::spawn(CHILD_TEST, "transport");
    child.expect_line(READY);
    child.signal("TERM");
    child.expect_line("app draining");
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

#[cfg(unix)]
#[test]
fn a_signal_before_any_app_exists_exits_at_once_naming_main() {
    let mut child = ChildProcess::spawn(CHILD_TEST, "main");
    child.expect_line(READY);
    child.signal("TERM");

    let (code, printed) = child.exit_within(Duration::from_secs(1));

    assert_eq!(code, Some(143), "SIGTERM's code: {printed:#?}");
    let said = child.stderr();
    let lines: Vec<&String> = said
        .iter()
        .filter(|line| {
            line.contains("shutdown signal received before the app started: exiting at once")
        })
        .collect();
    assert_eq!(lines.len(), 1, "one line, on stderr: {said:#?}");
    assert!(lines[0].contains(r#"step="main""#), "{}", lines[0]);
    assert!(lines[0].contains(r#"signal="SIGTERM""#), "{}", lines[0]);
    assert!(lines[0].contains("exit_code=143"), "{}", lines[0]);
}

const DURING_THE_BOOT: &str = "shutdown signal received during the boot: stopping it";

/// The child's boot, signalled once it reached its slow step: it stops within
/// a second, and what `main` got and the line it filed both name the step.
#[cfg(unix)]
fn signalled_during_the_boot(role: &str, step: &str) -> Vec<String> {
    let mut child = ChildProcess::spawn(CHILD_TEST, role);
    child.expect_line(READY);
    child.signal("TERM");

    let (code, printed) = child.exit_within(Duration::from_secs(1));

    assert_eq!(
        code,
        Some(0),
        "the boot returned its error to `main`: {printed:#?}"
    );
    let line = printed
        .iter()
        .find(|line| line.contains(DURING_THE_BOOT))
        .unwrap_or_else(|| panic!("the line naming the step: {printed:#?}"));
    assert!(line.contains(step), "{line}");
    assert!(line.contains(r#""SIGTERM""#), "{line}");
    let failed = printed
        .iter()
        .find(|line| line.contains(BOOT_FAILED))
        .unwrap_or_else(|| panic!("what `main` got: {printed:#?}"));
    assert!(
        failed.contains("interrupted by Terminate during"),
        "a `BootInterruptedError`: {failed}"
    );
    assert!(failed.contains(step), "{failed}");
    assert!(
        failed.contains("the boot was stopped by Terminate during"),
        "{failed}"
    );
    printed
}

#[cfg(unix)]
#[test]
fn a_signal_during_a_factory_stops_the_boot_naming_its_type() {
    signalled_during_the_boot("factory", "SlowPool");
}

#[cfg(unix)]
#[test]
fn a_signal_while_a_transport_configures_stops_the_boot_naming_it() {
    signalled_during_the_boot("configure", "ConfiguresSlowly");
}

#[cfg(unix)]
#[test]
fn a_signal_during_an_init_hook_stops_the_boot_naming_the_hook() {
    let printed = signalled_during_the_boot("init", "WarmsSlowly::warm");
    assert!(
        printed.iter().any(|line| line.contains(FIRST_INIT_RAN)),
        "another host's init hook ran first: {printed:#?}"
    );
    let torn_down: Vec<&String> = printed
        .iter()
        .filter(|line| line.contains(FIRST_TORN_DOWN) || line.contains(WARMING_TORN_DOWN))
        .collect();
    assert!(
        torn_down.len() == 2
            && torn_down[0].contains(WARMING_TORN_DOWN)
            && torn_down[1].contains(FIRST_TORN_DOWN),
        "the interrupted boot ran every host's shutdown hook, in reverse: {printed:#?}"
    );
}

#[cfg(unix)]
#[test]
fn a_second_signal_while_an_init_hook_blocks_its_thread_exits_at_once_naming_it() {
    let mut child = ChildProcess::spawn(CHILD_TEST, "blocking-init");
    child.expect_line(READY);
    child.signal("TERM");
    child.expect_line("app draining");
    child.signal("TERM");

    let (code, printed) = child.exit_within(Duration::from_secs(2));

    assert_eq!(code, Some(143), "SIGTERM's code: {printed:#?}");
    let line = printed
        .iter()
        .find(|line| line.contains(EXITING))
        .unwrap_or_else(|| panic!("the line naming what it abandons: {printed:#?}"));
    assert!(line.contains(r#""boot""#), "{line}");
    assert!(line.contains("BlocksOnInit"), "{line}");
    assert!(line.contains("migrate"), "{line}");
}

#[cfg(unix)]
#[test]
fn a_second_signal_while_a_factory_blocks_its_thread_exits_at_once_naming_its_type() {
    let mut child = ChildProcess::spawn(CHILD_TEST, "blocking-factory");
    child.expect_line(READY);
    child.signal("TERM");
    child.expect_line("app draining");
    child.signal("TERM");

    let (code, printed) = child.exit_within(Duration::from_secs(2));

    assert_eq!(code, Some(143), "SIGTERM's code: {printed:#?}");
    let line = printed
        .iter()
        .find(|line| line.contains(EXITING))
        .unwrap_or_else(|| panic!("the line naming what it abandons: {printed:#?}"));
    assert!(line.contains(r#""boot""#), "{line}");
    assert!(
        line.contains("type_name"),
        "the boot step's own field: {line}"
    );
    assert!(line.contains("SlowPool"), "{line}");
}

#[cfg(unix)]
#[test]
fn an_app_run_outside_the_kernels_main_still_stops_at_the_signal() {
    let mut child = ChildProcess::spawn(CHILD_TEST, "fallback");
    child.expect_line(READY);
    child.signal("INT");

    let (code, printed) = child.exit_within(Duration::from_secs(2));

    assert_eq!(
        code,
        Some(0),
        "the app stopped and `main` returned: {printed:#?}"
    );
    let draining = printed
        .iter()
        .find(|line| line.contains("app draining"))
        .unwrap_or_else(|| panic!("the transition line: {printed:#?}"));
    assert!(draining.contains("Interrupt"), "{draining}");
}
