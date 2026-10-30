//! The activation seam: import [`ScheduleModule`] and the framework attaches
//! the [`Scheduler`] to the app at boot.

use std::borrow::Cow;

use nest_rs_core::__private::{EdgeHosts, EdgeRuntime, attaches_transport};
use nest_rs_core::{Container, ContainerBuilder, Edge, Module, Registering, inventory};

use crate::{ScheduledMethod, Scheduler};

/// Activates the scheduler runtime for the app.
///
/// Every `#[scheduled]` method on a provider reachable from the app's module
/// tree fires under its declared trigger. A schedule adapter imports this
/// module bare beside the provider it lists, so an app importing the adapter
/// runs its ticks; a host the app reaches without it is named at `warn` when
/// the app runs.
///
/// A job declaring `replicas = "one"` also needs one occurrence lock binding
/// imported beside it — the boot names the job and
/// [`BACKEND_REMEDY`](crate::BACKEND_REMEDY) when none is.
pub struct ScheduleModule;

impl Module for ScheduleModule {
    // Registered once however many modules import it, so two importers attach
    // one scheduler, not two firing every tick twice.
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(Scheduler::new()))
    }
}

nest_rs_core::inventory::submit! {
    EdgeRuntime {
        edge: Edge::Schedule,
        import: "nest_rs::schedule::ScheduleModule",
        attached: attaches_transport::<Scheduler>,
        hosts: scheduled_hosts,
    }
}

/// Every `#[scheduled]` host this binary links.
fn scheduled_hosts(_: &Container) -> EdgeHosts {
    inventory::iter::<ScheduledMethod>()
        .map(|method| (method.provider_type_id(), Cow::Borrowed(method.provider())))
        .collect()
}
