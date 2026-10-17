use std::sync::Arc;

use nest_rs_core::{Collecting, Container, ContainerBuilder, Module, Registering};
use nest_rs_database::{BACKEND_REMEDY, UnitOfWork};
use sea_orm::DatabaseConnection;

use crate::module::{BudgetReach, SUBSTRATE_REMEDY, budgets};
use crate::unit::Settles;
use crate::{SeaOrmConfig, SeaOrmUnitOfWork};

/// Binds the `nest-rs-database` port: [`SeaOrmUnitOfWork`] as `dyn
/// UnitOfWork`, which every edge opens its units of work through, the
/// `DbContext` request interceptor for HTTP (feature `http`). A bare import beside
/// [`SeaOrmModule::for_root`](crate::SeaOrmModule::for_root).
pub struct SeaOrmDatabaseModule;

impl Module for SeaOrmDatabaseModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = budgets(BudgetReach::Ambient)
            .into_iter()
            .fold(builder, ContainerBuilder::provide_meta);
        builder
            // Queued after the pool's and config's factories, so a missing one
            // is named before any later factory dials.
            .provide_declared_factory_after::<
                Arc<dyn UnitOfWork>,
                (DatabaseConnection, SeaOrmConfig, Settles),
                _,
                _,
            >(BACKEND_REMEDY, |container| async move {
                Ok(Arc::new(unit_of_work(&container)?) as Arc<dyn UnitOfWork>)
            })
            // Installed after the pool, so its close drains every settle still
            // running before the pool's closes.
            .provide_factory_after::<Settles, (DatabaseConnection,), _, _>(|_| async {
                Ok(Settles::default())
            })
            .provide_close::<Settles, _, _>(|settles| async move {
                settles.drain().await;
                Ok(())
            })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        // A seeded `dyn UnitOfWork` skips the factory above, and its check.
        if let Err(missing) = substrate(&builder.snapshot()) {
            return builder.refuse(missing);
        }
        #[cfg(feature = "http")]
        let builder = <crate::DbContext as nest_rs_core::Discoverable>::register(builder);
        builder
    }
}

/// The pool and its config, checked present: the binding is built over both.
fn substrate(container: &Container) -> anyhow::Result<()> {
    unit_of_work(container).map(drop)
}

/// SeaORM's unit of work over the container's pool and config, its settles
/// drained by the binding's close.
fn unit_of_work(container: &Container) -> anyhow::Result<SeaOrmUnitOfWork> {
    let db = present::<DatabaseConnection>(container)?;
    let config = present::<SeaOrmConfig>(container)?;
    let settles = container
        .get::<Settles>()
        .map(|settles| (*settles).clone())
        .unwrap_or_default();
    Ok(SeaOrmUnitOfWork::draining_into(db, &config, settles))
}

fn present<T: std::any::Any + Send + Sync>(container: &Container) -> anyhow::Result<Arc<T>> {
    container.get::<T>().ok_or_else(|| {
        anyhow::anyhow!(
            "SeaOrmDatabaseModule: no `{}` in the container — {SUBSTRATE_REMEDY}",
            nest_rs_core::__private::short_type_name::<T>()
        )
    })
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, ContestedDeclarationError, LateFactoryError};
    use nest_rs_database::{OpenUnit, Unhonoured, UnitSpec};

    use super::*;

    /// Another driver's unit of work, bound the way a driver binds one.
    #[derive(Clone)]
    struct BareUnitOfWork;

    impl UnitOfWork for BareUnitOfWork {
        fn open(&self, _: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
            Err(Unhonoured::deterministic("never opened"))
        }

        fn settle_net(&self) -> std::time::Duration {
            std::time::Duration::from_secs(16)
        }
    }

    struct BareUnitOfWorkModule;

    impl Module for BareUnitOfWorkModule {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder
        }

        fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
            builder.provide_factory_dyn::<BareUnitOfWork, dyn UnitOfWork, _, _>(
                |_| async { Ok(BareUnitOfWork) },
                |binding| Arc::new(binding) as Arc<dyn UnitOfWork>,
            )
        }
    }

    #[tokio::test]
    async fn a_second_unit_of_work_binding_fails_the_boot_in_the_ports_words() {
        for seaorm_first in [true, false] {
            let app = if seaorm_first {
                App::builder()
                    .module::<SeaOrmDatabaseModule>()
                    .module::<BareUnitOfWorkModule>()
            } else {
                App::builder()
                    .module::<BareUnitOfWorkModule>()
                    .module::<SeaOrmDatabaseModule>()
            };
            let Err(refused) = app.build().await else {
                panic!("one binding would run every unit, the other none");
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
            refused.contains(": SeaOrmDatabaseModule: no `SeaOrmConfig` in the container")
                && refused.contains("`SeaOrmModule::for_root(None)`"),
            "the factory's own sentence, after the one naming it: {refused}"
        );
    }

    #[tokio::test]
    async fn a_unit_of_work_seeded_without_the_config_fails_the_boot_naming_the_module() {
        let seeded = SeaOrmUnitOfWork::new(
            Arc::new(DatabaseConnection::default()),
            &SeaOrmConfig::default(),
        );
        let Err(refused) = App::builder()
            .provide(DatabaseConnection::default())
            .provide_dyn::<dyn UnitOfWork>(Arc::new(seeded))
            .module::<SeaOrmDatabaseModule>()
            .build()
            .await
        else {
            panic!("a seeded unit of work still leaves the binding its substrate to read");
        };
        let refused = format!("{refused:#}");
        assert!(
            refused.starts_with("SeaOrmDatabaseModule: no `SeaOrmConfig` in the container"),
            "{refused}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_work_a_unit_held_for_its_commit_is_drained_before_the_pool_closes() {
        let app = App::builder()
            .provide(DatabaseConnection::default())
            .provide(SeaOrmConfig::default())
            .module::<SeaOrmDatabaseModule>()
            .build()
            .await
            .expect("the binding boots over a pool and its config");
        let binding = app
            .container()
            .get_dyn::<dyn UnitOfWork>()
            .expect("the binding");
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let unit = nest_rs_core::__private::declare_unit(
            "nest-rs-queue",
            "nest_rs::queue",
            nest_rs_core::operation_log::Kind::Consumer,
            "queue.job",
        );
        let started = tokio::time::Instant::now();
        let deadline = started + std::time::Duration::from_secs(1);
        nest_rs_database::run_unit(
            Some(&binding),
            UnitSpec::new(
                unit,
                nest_rs_database::ExecutorScope::Job,
                nest_rs_database::TransactionPolicy::PerUnit,
            )
            .with_deadline(deadline),
            async move {
                nest_rs_database::after_commit(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                })
                .await;
            },
            |()| true,
            |_| (),
        )
        .await;
        assert_eq!(started.elapsed(), std::time::Duration::from_secs(1));
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst), "it runs on");
        app.close().await;
        assert!(
            ran.load(std::sync::atomic::Ordering::SeqCst),
            "the binding's close drained it"
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
    async fn the_binding_imported_in_a_register_alone_fails_the_boot_naming_its_port() {
        let Err(refused) = App::builder()
            .provide(DatabaseConnection::default())
            .provide(SeaOrmConfig::default())
            .module::<RegistersOnly>()
            .build()
            .await
        else {
            panic!("every unit would run bare, in silence");
        };
        let late = refused
            .downcast_ref::<LateFactoryError>()
            .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
        assert_eq!(late.type_name, std::any::type_name::<Arc<dyn UnitOfWork>>());
    }
}
