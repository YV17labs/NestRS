use std::sync::Arc;

#[cfg(any(feature = "http", feature = "graphql"))]
use anyhow::Context as _;
use nest_rs_config::ConfigModule;
use nest_rs_core::{Collecting, Container, ContainerBuilder, Module, Registering};
use nest_rs_database::{UnitOfWork, provide_unit_of_work};
use sea_orm::DatabaseConnection;

use crate::SeaOrmModule;
use crate::module::{BudgetReach, budgets};
#[cfg(any(feature = "http", feature = "graphql"))]
use crate::unique_key::UniqueKeys;
use crate::unit::Settles;
use crate::{SeaOrmConfig, SeaOrmUnitOfWork};

/// Binds the `nest-rs-database` port: [`SeaOrmUnitOfWork`] as `dyn
/// UnitOfWork`, which every edge opens its units of work through and every
/// lifecycle hook runs in. A bare import, which imports the pool it runs on,
/// [`SeaOrmModule`]: an app names that one only to pin it.
///
/// Under an edge rendering a `#[crud]` write (`http`, `graphql`), it reads at boot,
/// from the catalog, the unique keys of every `#[expose]` entity's table: a write
/// a unique key refuses names the input's fields that key holds, with no statement
/// on the request's path.
///
/// Under the `queue` feature it also keeps the queue port's outbox:
/// `queue::SeaOrmOutbox` as `dyn OutboxStore`, written by
/// every unit's transaction, and the relay's `OutboxModule`, which files what
/// a committed unit wrote on whichever queue backend the app binds.
pub struct SeaOrmDatabaseModule;

/// What the unit of work is built from: the pool, its config, the settles the
/// close drains — and the outbox its transactions write into.
#[cfg(not(feature = "queue"))]
type UnitInputs = (DatabaseConnection, SeaOrmConfig, Settles);
#[cfg(feature = "queue")]
type UnitInputs = (
    DatabaseConnection,
    SeaOrmConfig,
    Settles,
    crate::queue::SeaOrmOutbox,
);

impl Module for SeaOrmDatabaseModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = budgets(BudgetReach::Ambient)
            .into_iter()
            .fold(builder, ContainerBuilder::provide_meta);
        let builder = provide_unit_of_work::<UnitInputs, _, _>(builder, |container| async move {
            let db = present::<DatabaseConnection>(&container)?;
            let config = ConfigModule::resolved::<SeaOrmConfig>(&container)?;
            let settles = present::<Settles>(&container)?;
            let unit_of_work =
                SeaOrmUnitOfWork::draining_into(Arc::clone(&db), &config, (*settles).clone());
            #[cfg(feature = "queue")]
            let unit_of_work = unit_of_work.keeping(container.get::<crate::queue::SeaOrmOutbox>());
            #[cfg(any(feature = "http", feature = "graphql"))]
            let unit_of_work = {
                let keys = UniqueKeys::read(&db, unit_of_work.settle_net())
                    .await
                    .context(
                        "SeaOrmDatabaseModule: could not read the unique keys of the \
                         `#[expose]` entities' tables from the database's catalog",
                    )?;
                unit_of_work.reading(keys)
            };
            Ok(Arc::new(unit_of_work) as Arc<dyn UnitOfWork>)
        })
        // Installed after the pool, so its close drains every settle still
        // running before the pool's closes.
        .provide_factory_after::<Settles, (DatabaseConnection,), _, _>(|_| async {
            Ok(Settles::default())
        })
        .provide_close::<Settles, _, _>(|settles| async move {
            settles.drain().await;
            Ok(())
        });
        #[cfg(feature = "queue")]
        let builder = crate::queue::collect(builder);
        builder.import::<SeaOrmModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        #[cfg(feature = "queue")]
        let builder = crate::queue::register(builder);
        builder.import::<SeaOrmModule>()
    }
}

fn present<T: std::any::Any + Send + Sync>(container: &Container) -> anyhow::Result<Arc<T>> {
    container.get::<T>().ok_or_else(|| {
        anyhow::anyhow!(
            "SeaOrmDatabaseModule: `{}` was not resolved",
            nest_rs_core::__private::short_type_name::<T>()
        )
    })
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, ContestedDeclarationError, LateFactoryError};
    use nest_rs_database::{BACKEND_REMEDY, OpenUnit, Unhonoured, UnitSpec};

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

    #[cfg(feature = "queue")]
    #[tokio::test]
    async fn the_binding_keeps_the_outbox_and_attaches_the_relay_that_drains_it() {
        let app = App::builder()
            .provide(DatabaseConnection::default())
            .provide(SeaOrmConfig::default())
            .module::<SeaOrmDatabaseModule>()
            .build()
            .await
            .expect("the binding boots over a pool and its config");
        assert!(
            app.container()
                .get_dyn::<dyn nest_rs_database::OutboxStore>()
                .is_some()
        );
        assert!(
            app.container()
                .get::<nest_rs_queue::OutboxConfig>()
                .is_some(),
            "the relay's settings are read for it"
        );
        let attached: Vec<String> = nest_rs_core::Discovery::new(app.container())
            .meta::<nest_rs_core::TransportContribution>()
            .iter()
            .map(|contribution| contribution.meta.name().to_owned())
            .collect();
        assert_eq!(attached, ["outbox"]);
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
