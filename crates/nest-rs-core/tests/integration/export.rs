//! Covers `src/export.rs` — `App::run` refusing an OTLP endpoint the
//! deployment sets while the process logs through the kernel's console alone.
//! The environment and the global subscriber are process-wide, so each case
//! runs in a [`ChildProcess`] of its own.

use std::time::Duration;

use nest_rs_core::{App, UnservedExportError, module};

use crate::ChildProcess;

const CHILD_TEST: &str = "export::export_child_process";

/// What a child prints once `run` returned.
const RAN: &str = "EXPORT-CHILD RAN";

/// What a child prints, before the error's chain, once `run` refused.
const REFUSED: &str = "EXPORT-CHILD REFUSED";

/// What a child prints once the boot alone returned an app.
const BUILT: &str = "EXPORT-CHILD BUILT";

/// Every variable the check reads, blanked so a child reads what its test set,
/// whatever the shell running the suite exports.
const READ: [&str; 8] = [
    "OTEL_SDK_DISABLED",
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
    "OTEL_TRACES_EXPORTER",
    "OTEL_METRICS_EXPORTER",
    "OTEL_LOGS_EXPORTER",
];

#[module]
struct Bare;

#[test]
fn export_child_process() {
    match crate::child_role().as_deref() {
        Some("run") => run(false),
        Some("run-own-subscriber") => run(true),
        Some("build") => build(),
        _ => {}
    }
}

/// `App::run` on an app serving nothing, which stops for want of a transport
/// once it booted — after a global subscriber of the app's own when `own`.
#[expect(
    clippy::print_stdout,
    reason = "the child's stdout is the protocol its parent test reads"
)]
fn run(own: bool) {
    if own {
        tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default())
            .expect("the child's first subscriber");
    }
    let ran =
        runtime().block_on(async { App::builder().module::<Bare>().build().await?.run().await });
    match ran {
        Ok(()) => println!("{RAN}"),
        Err(error) => {
            let typed = error.downcast_ref::<UnservedExportError>().is_some();
            println!("{REFUSED} typed={typed}: {error:#}");
        }
    }
}

/// The boot a suite runs, which never reaches `run`.
#[expect(
    clippy::print_stdout,
    reason = "the child's stdout is the protocol its parent test reads"
)]
fn build() {
    match runtime().block_on(App::builder().module::<Bare>().build()) {
        Ok(_) => println!("{BUILT}"),
        Err(error) => println!("{REFUSED}: {error:#}"),
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime the boot runs on")
}

/// What a child in `role` said under `set`, every other variable the check
/// reads blank.
fn said_under(role: &str, set: &[(&str, &str)]) -> String {
    let mut env: Vec<(String, String)> = READ
        .iter()
        .map(|name| ((*name).to_owned(), String::new()))
        .collect();
    for (name, value) in set {
        if let Some(slot) = env.iter_mut().find(|(variable, _)| variable == name) {
            slot.1 = (*value).to_owned();
        }
    }
    let mut child = ChildProcess::spawn_with(CHILD_TEST, role, &env);
    let (code, printed) = child.exit_within(Duration::from_secs(10));
    assert_eq!(code, Some(0), "the child ran: {printed:#?}");
    printed
        .into_iter()
        .find(|line| line.starts_with(RAN) || line.starts_with(REFUSED) || line.starts_with(BUILT))
        .unwrap_or_else(|| panic!("the child said how its boot went"))
}

/// The deployment asks for an export the binary never installed: `run`
/// refuses, naming the variable and the line to add, never the endpoint.
#[test]
fn run_refuses_an_endpoint_the_kernel_s_console_alone_cannot_serve() {
    for variable in [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
    ] {
        let said = said_under("run", &[(variable, "http://collector:4318")]);
        assert!(said.starts_with(&format!("{REFUSED} typed=true")), "{said}");
        assert!(said.contains(variable), "{said}");
        assert!(said.contains("OpenTelemetry::init()"), "{said}");
        assert!(
            !said.contains("collector"),
            "the endpoint is never quoted: {said}"
        );
    }
}

/// The specification's switches turn the export off, so nothing is asked.
#[test]
fn run_runs_an_endpoint_the_deployment_switched_off() {
    let base = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318");
    assert_eq!(
        said_under("run", &[base, ("OTEL_SDK_DISABLED", "true")]),
        RAN
    );
    assert_eq!(
        said_under(
            "run",
            &[
                base,
                ("OTEL_TRACES_EXPORTER", "none"),
                ("OTEL_METRICS_EXPORTER", "none"),
                ("OTEL_LOGS_EXPORTER", "none"),
            ]
        ),
        RAN
    );
    assert_eq!(said_under("run", &[]), RAN, "no endpoint, nothing asked");
}

/// An app that installed a subscriber of its own is its own to judge: the
/// kernel cannot see whether it exports.
#[test]
fn run_runs_an_endpoint_beside_a_subscriber_of_the_app_s_own() {
    assert_eq!(
        said_under(
            "run-own-subscriber",
            &[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318")]
        ),
        RAN
    );
}

/// A suite boots without `run`, so a `.env` naming a local collector never
/// refuses it.
#[test]
fn the_boot_a_suite_runs_never_refuses_an_endpoint() {
    assert_eq!(
        said_under(
            "build",
            &[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318")]
        ),
        BUILT
    );
}
