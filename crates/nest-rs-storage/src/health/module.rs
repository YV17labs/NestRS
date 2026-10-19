//! [`StorageHealthModule`] — the storage binding of the health port.

use std::any::TypeId;

use nest_rs_core::{ContainerBuilder, Module, Registering};
use nest_rs_health::{IndicatorEntry, ProbeKind};

use super::indicator::{NAME, listed, present};
use crate::Storage;

/// Gates the readiness and startup probes on the store answering a page of its
/// bucket's listing — the boot's proof, one attempt per probe — under the
/// name `storage`. A bare import beside `HealthModule` and
/// [`StorageModule`](crate::StorageModule); liveness never waits on it. A
/// store refusing the listing itself (a policy granting no `s3:ListBucket`)
/// reads up, as it boots, the probe first meeting the refusal filing the
/// boot's `warn`; a test overriding [`Storage`] probes nothing.
pub struct StorageHealthModule;

impl Module for StorageHealthModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        // The override opens no store: there is nothing of the app's to probe.
        if nest_rs_core::__private::overrides_any(&builder, &[TypeId::of::<Storage>()]) {
            return builder;
        }
        if let Err(missing) = present(&builder.snapshot()) {
            return builder.refuse(missing);
        }
        builder
            .provide_meta(IndicatorEntry::new::<Self>(
                NAME,
                ProbeKind::Readiness,
                listed,
            ))
            .provide_meta(IndicatorEntry::new::<Self>(
                NAME,
                ProbeKind::Startup,
                listed,
            ))
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, module};
    use nest_rs_health::{HealthModule, HealthService, IndicatorStatus};

    use super::*;
    use crate::{StorageConfig, StorageModule};

    fn in_memory() -> StorageConfig {
        nest_rs_testing::load_project_env();
        StorageConfig {
            url: "memory:///".into(),
            ..StorageConfig::default()
        }
    }

    #[module(imports = [HealthModule, StorageModule, StorageHealthModule])]
    struct ProbedStore;

    #[tokio::test]
    async fn the_store_answers_the_readiness_and_startup_probes_under_the_crate_s_name() {
        let app = App::builder()
            .provide(in_memory())
            .module::<ProbedStore>()
            .build()
            .await
            .unwrap_or_else(|refused| panic!("boots: {refused:#}"));
        let health = app
            .container()
            .get::<HealthService>()
            .expect("HealthModule provides it");
        for kind in [ProbeKind::Readiness, ProbeKind::Startup] {
            let report = health.probe(kind).await;
            assert_eq!(report.status, IndicatorStatus::Up, "{kind:?}: {report:#?}");
            assert_eq!(
                report
                    .details
                    .keys()
                    .map(|key| &**key)
                    .collect::<Vec<&str>>(),
                ["storage"],
                "{kind:?}: {report:#?}"
            );
        }
        assert!(
            health.probe(ProbeKind::Liveness).await.details.is_empty(),
            "a store outage is no reason to restart the process"
        );
    }

    #[module(imports = [HealthModule, StorageHealthModule])]
    struct ProbedWithoutTheStore;

    #[tokio::test]
    async fn without_the_store_the_boot_names_the_import_that_opens_it() {
        let Err(refused) = App::builder()
            .module::<ProbedWithoutTheStore>()
            .build()
            .await
        else {
            panic!("an indicator over no store must not boot");
        };
        let sentence = format!("{refused:#}");
        assert!(
            sentence.contains("StorageHealthModule") && sentence.contains("StorageModule"),
            "{sentence}"
        );
    }
}
