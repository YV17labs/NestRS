//! The activation seam: import [`ScheduleModule`] and the framework attaches
//! the [`Scheduler`] to the app at boot.

use nest_rs_core::{ContainerBuilder, Module, Registering, TransportContribution};

use crate::Scheduler;

/// Activates the scheduler runtime for the app.
///
/// Every `#[scheduled]` method on a provider reachable from the app's module
/// tree fires under its declared trigger; without this import, none ticks.
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
