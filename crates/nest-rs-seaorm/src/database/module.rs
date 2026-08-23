//! [`SeaOrmDatabaseModule`] — the binding of the `nest-rs-database` port:
//! SeaORM's [`Executor`](crate::Executor) installed around every unit of work.
//! A bare import beside [`SeaOrmModule::for_root`](crate::SeaOrmModule::for_root),
//! which opens the pool it reads.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{ContainerBuilder, Module};
use sea_orm::DatabaseConnection;

use crate::module::POOL_REMEDY;

/// Binds the ambient executor: the `DbContext` request interceptor for HTTP
/// (feature `http`), the `WorkerDbContext as dyn JobContext` bridge for jobs,
/// and the link-time audits this binding brings.
pub struct SeaOrmDatabaseModule;

impl Module for SeaOrmDatabaseModule {
    // A hand-written `impl Module` dedupes itself, as `#[module]` does for its
    // expansions: two importers of this binding must install one interceptor
    // and one audit, not two — the second `DbContext` wrap would open a second
    // transaction per request, in silence.
    fn collect(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_collected(TypeId::of::<Self>()) {
            return builder;
        }
        // The worker bridge is a factory output so it counts as global
        // infrastructure for every transport that runs jobs — and a factory
        // declared *after* the pool's, so the one thing it can fail on is the
        // pool being absent, which it then names.
        builder.provide_factory_dyn_after::<
            crate::WorkerDbContext,
            dyn nest_rs_worker::JobContext,
            DatabaseConnection,
            _,
            _,
        >(
            |container| async move {
                if container.get::<DatabaseConnection>().is_none() {
                    anyhow::bail!("SeaOrmDatabaseModule: {POOL_REMEDY}");
                }
                Ok(crate::WorkerDbContext::from_container(&container))
            },
            |context| Arc::new(context) as Arc<dyn nest_rs_worker::JobContext>,
        )
    }

    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_registered(TypeId::of::<Self>()) {
            return builder;
        }
        install_boot_audits(install_request_layers(builder))
    }
}

/// Install the sync request layer: the `DbContext` HTTP interceptor. Built
/// eagerly from the snapshot — the pool is a factory output present before the
/// register phase: its absence failed the async boot in `collect`, and the
/// synchronous `App::new` refuses the queued factory before reaching here.
fn install_request_layers(builder: ContainerBuilder) -> ContainerBuilder {
    // The `DbContext` interceptor only exists with the `http` feature (it is the
    // HTTP request seam). Without it there is no HTTP layer to install — the
    // worker bridge queued in `collect` still applies.
    #[cfg(feature = "http")]
    let builder = <crate::DbContext as nest_rs_core::Discoverable>::register(builder);
    builder
}

/// Install the link-time invariant checks this import brings — providers that
/// need neither config nor pool, only what the decorators submitted, and that
/// refuse boot from `#[on_module_init]`.
///
/// Separate from the request layers above: these run once and touch no request.
/// The audits ride `SeaOrmDatabaseModule` because an app without it has no
/// `CrudService` to mis-wire; an app composing the ORM some other way calls the
/// audit directly (`audit_soft_delete_bindings`).
fn install_boot_audits(builder: ContainerBuilder) -> ContainerBuilder {
    <crate::soft_delete::SoftDeleteAudit as nest_rs_core::Discoverable>::register(builder)
}
