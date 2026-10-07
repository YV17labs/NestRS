//! The activation seam: import [`ScheduleModule`] in an `#[module(imports =
//! [...])]` and the framework attaches the [`Scheduler`] to
//! the app at boot.
//!
//! The module is hand-written `impl Module` (rather than `#[module]`) because
//! its sole job is to contribute a `TransportContribution` via
//! `provide_meta` — it owns no provider and exposes no injectable.

use nest_rs_core::{ContainerBuilder, Module, Registering, TransportContribution};

use crate::Scheduler;

/// Activates the scheduler runtime for the app.
///
/// Importing this module turns on the scheduler at boot: every
/// `#[scheduled]` method on a provider reachable from the app's module
/// tree fires under its declared trigger. Without this import,
/// `#[scheduled]` methods compile in but never tick.
///
/// A job declaring `replicas = "one"` also needs one occurrence lock binding
/// imported beside it — the boot names the job and
/// [`BACKEND_REMEDY`](crate::BACKEND_REMEDY) when none is.
pub struct ScheduleModule;

impl Module for ScheduleModule {
    // Registered once however many modules import it, so two importers attach
    // one scheduler, not two firing every tick twice.
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "Scheduler",
            build: |_| Ok(Box::new(Scheduler::new())),
        })
    }
}
