//! [`SeaOrmDatabaseModule`] — the binding of the `nest-rs-database` port:
//! SeaORM's [`Executor`](crate::Executor) installed around every unit of work.
//! A bare import beside [`SeaOrmModule::for_root`](crate::SeaOrmModule::for_root),
//! which opens the pool it reads.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{Container, ContainerBuilder, Module};
use sea_orm::DatabaseConnection;

use crate::SeaOrmConfig;
use crate::module::{BudgetReach, SUBSTRATE_REMEDY, pool_budget};

/// Binds the ambient executor: the `DbContext` request interceptor for HTTP
/// (feature `http`), the `WorkerDbContext as dyn JobContext` bridge for jobs,
/// and the link-time audits this binding brings. Code anywhere then reaches the
/// pool through `Repo`, so the binding declares its budget ambient: every net
/// around developer code holds it.
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
        builder = builder.provide_meta(pool_budget(BudgetReach::Ambient));
        // The worker bridge is a factory output so it counts as global
        // infrastructure for every transport that runs jobs — and a factory
        // declared *after* the pool's and its config's, so what it can fail on
        // is either being absent, which it then names before `register` builds
        // `DbContext` over both.
        builder.provide_factory_dyn_after_both::<
            crate::WorkerDbContext,
            dyn nest_rs_worker::JobContext,
            DatabaseConnection,
            SeaOrmConfig,
            _,
            _,
        >(
            |container| async move {
                substrate::<DatabaseConnection>(&container)?;
                substrate::<SeaOrmConfig>(&container)?;
                Ok(crate::WorkerDbContext::from_container(&container))
            },
            |context| Arc::new(context) as Arc<dyn nest_rs_worker::JobContext>,
        )
    }

    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_registered(TypeId::of::<Self>()) {
            return builder;
        }
        // The `DbContext` interceptor only exists with the `http` feature (it is
        // the HTTP request seam). Built eagerly from the snapshot — the pool and
        // its config are present before the register phase: their absence
        // failed the async boot in the factory above, and the synchronous
        // `App::new` refuses the queued factory before reaching here.
        #[cfg(feature = "http")]
        let builder = <crate::DbContext as nest_rs_core::Discoverable>::register(builder);
        // The link-time invariant checks this import brings — providers that
        // need neither config nor pool, only what the decorators submitted, and
        // that refuse boot from `#[on_module_init]`. They ride
        // `SeaOrmDatabaseModule` because an app without it has no `CrudService`
        // to mis-wire; an app composing the ORM some other way calls the audit
        // directly (`audit_soft_delete_bindings`).
        <crate::soft_delete::SoftDeleteAudit as nest_rs_core::Discoverable>::register(builder)
    }
}

/// Read for its absence: `from_container` would panic on it, and this is what
/// makes it the named boot error every binding gives.
fn substrate<T: std::any::Any + Send + Sync>(container: &Container) -> anyhow::Result<()> {
    container.get::<T>().map(drop).ok_or_else(|| {
        anyhow::anyhow!(
            "SeaOrmDatabaseModule: no `{}` in the container — {SUBSTRATE_REMEDY}",
            nest_rs_core::short_type_name::<T>()
        )
    })
}

#[cfg(test)]
mod tests {
    use nest_rs_core::App;

    use super::*;

    #[tokio::test]
    async fn a_pool_seeded_without_its_module_fails_the_boot_naming_the_module() {
        let Err(refused) = App::builder()
            .provide(DatabaseConnection::default())
            .module::<SeaOrmDatabaseModule>()
            .build()
            .await
        else {
            panic!("the binding reads the config `SeaOrmModule` resolves beside its pool");
        };
        let refused = format!("{refused:#}");
        assert!(
            refused.starts_with("SeaOrmDatabaseModule: no `SeaOrmConfig` in the container")
                && refused.contains("`SeaOrmModule::for_root(None)`"),
            "{refused}"
        );
    }
}
