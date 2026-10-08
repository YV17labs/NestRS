use std::sync::Arc;

use nest_rs_core::{Collecting, Container, ContainerBuilder, Module, Registering};
use nest_rs_worker::{BACKEND_REMEDY, JobContext};
use sea_orm::DatabaseConnection;

use crate::SeaOrmConfig;
use crate::module::{BudgetReach, SUBSTRATE_REMEDY, budgets};

/// Binds the ambient executor: the `DbContext` request interceptor for HTTP
/// (feature `http`), the `WorkerDbContext as dyn JobContext` bridge for jobs,
/// and the link-time audits this binding brings. A bare import beside
/// [`SeaOrmModule::for_root`](crate::SeaOrmModule::for_root).
pub struct SeaOrmDatabaseModule;

impl Module for SeaOrmDatabaseModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = budgets(BudgetReach::Ambient)
            .into_iter()
            .fold(builder, ContainerBuilder::provide_meta);
        // Queued after the pool's and config's factories, so a missing one is
        // named before any later factory dials.
        builder.provide_declared_factory_after_both::<
            Arc<dyn JobContext>,
            DatabaseConnection,
            SeaOrmConfig,
            _,
            _,
        >(BACKEND_REMEDY, |container| async move {
            substrate(&container)?;
            Ok(Arc::new(crate::WorkerDbContext::from_container(&container)) as Arc<dyn JobContext>)
        })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        // A seeded `dyn JobContext` skips the factory above, and its check.
        if let Err(missing) = substrate(&builder.snapshot()) {
            return builder.refuse(missing);
        }
        #[cfg(feature = "http")]
        let builder = <crate::DbContext as nest_rs_core::Discoverable>::register(builder);
        <crate::soft_delete::SoftDeleteAudit as nest_rs_core::Discoverable>::register(builder)
    }
}

/// The pool and its config, checked present: `from_container` panics on either.
fn substrate(container: &Container) -> anyhow::Result<()> {
    present::<DatabaseConnection>(container)?;
    present::<SeaOrmConfig>(container)
}

fn present<T: std::any::Any + Send + Sync>(container: &Container) -> anyhow::Result<()> {
    container.get::<T>().map(drop).ok_or_else(|| {
        anyhow::anyhow!(
            "SeaOrmDatabaseModule: no `{}` in the container — {SUBSTRATE_REMEDY}",
            nest_rs_core::short_type_name::<T>()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;

    use nest_rs_core::{App, ContestedDeclarationError, LateFactoryError};
    use nest_rs_worker::{JobSettlement, JobTransaction};

    use super::*;

    /// Another bridge's context, bound the way a driver binds a trait object.
    #[derive(Clone)]
    struct BareContext;

    impl JobContext for BareContext {
        fn scope<'a>(
            &'a self,
            _: JobTransaction,
            inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
        ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>> {
            Box::pin(async move {
                inner.await;
                JobSettlement::Settled
            })
        }
    }

    struct BareContextModule;

    impl Module for BareContextModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }

        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory_dyn::<BareContext, dyn JobContext, _, _>(
                |_| async { Ok(BareContext) },
                |context| Arc::new(context) as Arc<dyn JobContext>,
            )
        }
    }

    #[tokio::test]
    async fn a_second_job_context_binding_fails_the_boot_in_the_ports_words() {
        for seaorm_first in [true, false] {
            let app = if seaorm_first {
                App::builder()
                    .module::<SeaOrmDatabaseModule>()
                    .module::<BareContextModule>()
            } else {
                App::builder()
                    .module::<BareContextModule>()
                    .module::<SeaOrmDatabaseModule>()
            };
            let Err(refused) = app.build().await else {
                panic!("one context would run every job, the other none");
            };
            let contested = refused
                .downcast_ref::<ContestedDeclarationError>()
                .unwrap_or_else(|| panic!("not the contest: {refused:#}"));
            assert_eq!(
                contested.remedy, BACKEND_REMEDY,
                "seaorm first: {seaorm_first}"
            );
        }
    }

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

    #[tokio::test]
    async fn a_job_context_seeded_without_the_config_fails_the_boot_naming_the_module() {
        let pool = Container::builder()
            .provide(DatabaseConnection::default())
            .build();
        let Err(refused) = App::builder()
            .provide(DatabaseConnection::default())
            .provide_dyn::<dyn JobContext>(Arc::new(crate::WorkerDbContext::from_container(&pool)))
            .module::<SeaOrmDatabaseModule>()
            .build()
            .await
        else {
            panic!("a seeded job context still leaves the interceptor its config to read");
        };
        let refused = format!("{refused:#}");
        assert!(
            refused.starts_with("SeaOrmDatabaseModule: no `SeaOrmConfig` in the container"),
            "{refused}"
        );
    }

    /// A hand-written importer that imports the binding in its register alone.
    struct RegistersOnly;

    impl Module for RegistersOnly {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<SeaOrmDatabaseModule>()
        }
    }

    #[tokio::test]
    async fn the_binding_imported_in_a_register_alone_fails_the_boot_naming_its_context() {
        let Err(refused) = App::builder()
            .provide(DatabaseConnection::default())
            .provide(SeaOrmConfig::default())
            .module::<RegistersOnly>()
            .build()
            .await
        else {
            panic!("every job would run bare, in silence");
        };
        let late = refused
            .downcast_ref::<LateFactoryError>()
            .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
        assert_eq!(late.type_name, std::any::type_name::<Arc<dyn JobContext>>());
    }
}
