//! The activation seam: import [`ScheduleModule`] in an `#[module(imports =
//! [...])]` and the framework attaches the [`Scheduler`](crate::Scheduler) to
//! the app at boot.
//!
//! The module is hand-written `impl Module` (rather than `#[module]`) because
//! its sole job is to contribute a `TransportContribution` via
//! `provide_meta` — it owns no provider and exposes no injectable.

use nest_rs_core::{ContainerBuilder, Module, TransportContribution};

use crate::Scheduler;

/// Activates the scheduler runtime for the app.
///
/// Importing this module turns on the scheduler at boot: every
/// `#[scheduled]` method on a provider reachable from the app's module
/// tree fires under its declared trigger. Without this import,
/// `#[scheduled]` methods compile in but never tick.
pub struct ScheduleModule;

impl Module for ScheduleModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "Scheduler",
            build: |_| Ok(Box::new(Scheduler::new())),
        })
    }
}
