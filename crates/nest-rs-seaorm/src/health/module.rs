use nest_rs_core::{ContainerBuilder, Module, Registering};
use nest_rs_health::{IndicatorEntry, ProbeKind};

use super::indicator::{NAME, ping, present};

/// Binds the health port: attaches the pool's indicator, `seaorm`, so
/// `/health/ready` and `/health/startup` gate on a database round-trip. A bare
/// import beside [`SeaOrmModule::for_root`](crate::SeaOrmModule::for_root) and
/// `HealthModule`.
pub struct SeaOrmHealthModule;

impl Module for SeaOrmHealthModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        if let Err(missing) = present(&builder.snapshot()) {
            return builder.refuse(missing);
        }
        builder
            .provide_meta(IndicatorEntry::new::<Self>(
                NAME,
                ProbeKind::Readiness,
                ping,
            ))
            .provide_meta(IndicatorEntry::new::<Self>(NAME, ProbeKind::Startup, ping))
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, module};
    use nest_rs_health::{HealthModule, HealthService};
    use sea_orm::DatabaseConnection;

    use super::*;

    #[module(imports = [HealthModule, SeaOrmHealthModule])]
    struct ProbedPool;

    #[tokio::test]
    async fn the_pool_answers_the_readiness_and_startup_probes_under_the_crate_s_name() {
        let app = App::builder()
            .provide(DatabaseConnection::default())
            .module::<ProbedPool>()
            .build()
            .await
            .unwrap_or_else(|refused| panic!("boots: {refused:#}"));
        let health = app
            .container()
            .get::<HealthService>()
            .expect("HealthModule provides it");
        for kind in [ProbeKind::Readiness, ProbeKind::Startup] {
            let report = health.probe(kind).await;
            assert_eq!(
                report
                    .details
                    .keys()
                    .map(|key| &**key)
                    .collect::<Vec<&str>>(),
                ["seaorm"],
                "{kind:?}: {report:#?}"
            );
        }
        assert!(
            health.probe(ProbeKind::Liveness).await.details.is_empty(),
            "a database outage is no reason to restart the process"
        );
    }

    #[tokio::test]
    async fn without_the_pool_the_boot_names_the_import_that_opens_it() {
        let Err(refused) = App::builder().module::<ProbedPool>().build().await else {
            panic!("an indicator over no pool must not boot");
        };
        let sentence = format!("{refused:#}");
        assert!(
            sentence.contains("SeaOrmHealthModule")
                && sentence.contains("SeaOrmModule::for_root(None)"),
            "{sentence}"
        );
    }
}
