use std::sync::OnceLock;

use nest_rs_core::{Container, ReachableProviders, injectable, inventory};

use crate::indicator::{HealthIndicator, IndicatorReport, IndicatorStatus, ProbeKind, ProbeReport};

/// Aggregates every reachable [`HealthIndicator`] submitted via `#[indicators]`
/// into a per-probe [`ProbeReport`]. Apps don't usually touch this directly —
/// they register indicators and the [`crate::HealthController`] consumes the
/// reports.
///
/// A probe with zero indicators reports `up` with an empty body: importing
/// only `HealthModule` keeps the default permissive answer Kubernetes expects
/// before any custom check is wired in.
#[injectable]
#[derive(Default)]
pub struct HealthService {
    /// Set once at [`OnApplicationBootstrap`][1] by `HealthModule` so the
    /// service can resolve the per-indicator providers at probe time. The
    /// container is `Clone` (Arcs internally), so this carries a cheap handle.
    ///
    /// [1]: nest_rs_core::LifecyclePhase::OnApplicationBootstrap
    container: OnceLock<Container>,
}

impl HealthService {
    pub(crate) fn install_container(&self, container: Container) {
        let _ = self.container.set(container);
    }

    pub async fn probe(&self, kind: ProbeKind) -> ProbeReport {
        let Some(container) = self.container.get() else {
            // Called before bootstrap — no indicators can run; report `up`
            // so a probe that races the framework's wire-up does not flap.
            return ProbeReport::empty_up();
        };

        let reachable = container.get::<ReachableProviders>();
        let mut reports: Vec<IndicatorReport> = Vec::new();

        for entry in inventory::iter::<HealthIndicator>() {
            if entry.kind != kind {
                continue;
            }
            let provider_id = (entry.provider_type_id)();
            if let Some(r) = reachable.as_ref()
                && !r.0.contains(&provider_id)
            {
                tracing::warn!(
                    target: "nest_rs::health",
                    indicator = entry.name,
                    ?kind,
                    "skipped indicator: provider unreachable from app's module tree",
                );
                continue;
            }

            let (status, error) = match (entry.run)(container).await {
                Ok(()) => (IndicatorStatus::Up, None),
                Err(err) => {
                    let detail = format!("{err:#}");
                    tracing::warn!(
                        target: "nest_rs::health",
                        indicator = entry.name,
                        ?kind,
                        error = %detail,
                        "health indicator failed",
                    );
                    (IndicatorStatus::Down, Some("check failed".into()))
                }
            };
            reports.push(IndicatorReport {
                name: entry.name,
                status,
                error,
            });
        }

        if reports.is_empty() {
            ProbeReport::empty_up()
        } else {
            ProbeReport::from_indicators(reports)
        }
    }
}

#[cfg(test)]
mod tests {
    //! Drives the inventory-aggregation path through the crate-private
    //! `install_container` seam — the equivalent of the wired-up bootstrap
    //! hook but without booting an app.

    use super::*;
    use nest_rs_core::Container;

    struct UpHost;
    struct DownHost;

    impl UpHost {
        async fn ping(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }
    impl DownHost {
        async fn ping(&self) -> anyhow::Result<()> {
            anyhow::bail!("simulated outage")
        }
    }

    nest_rs_core::inventory::submit! {
        HealthIndicator {
            name: "up_host",
            kind: ProbeKind::Readiness,
            provider_type_id: || std::any::TypeId::of::<UpHost>(),
            run: |c| Box::pin(async move {
                c.get::<UpHost>().expect("UpHost registered").ping().await
            }),
        }
    }

    nest_rs_core::inventory::submit! {
        HealthIndicator {
            name: "down_host",
            kind: ProbeKind::Readiness,
            provider_type_id: || std::any::TypeId::of::<DownHost>(),
            run: |c| Box::pin(async move {
                c.get::<DownHost>().expect("DownHost registered").ping().await
            }),
        }
    }

    #[tokio::test]
    async fn aggregates_indicators_into_info_and_error_buckets() {
        let container = Container::builder()
            .provide(UpHost)
            .provide(DownHost)
            .build();
        let svc = HealthService::default();
        svc.install_container(container);

        let report = svc.probe(ProbeKind::Readiness).await;
        assert_eq!(report.status, IndicatorStatus::Down);
        assert_eq!(report.info.len(), 1);
        assert!(report.info.contains_key("up_host"));
        assert_eq!(report.error.len(), 1);
        let down = report
            .error
            .get("down_host")
            .expect("down_host in error bucket");
        assert_eq!(down.status, IndicatorStatus::Down);
        assert_eq!(
            down.error.as_deref(),
            Some("check failed"),
            "public probe responses must not leak indicator internals",
        );
        assert_eq!(report.details.len(), 2);
    }

    #[tokio::test]
    async fn other_probes_ignore_readiness_indicators() {
        let container = Container::builder()
            .provide(UpHost)
            .provide(DownHost)
            .build();
        let svc = HealthService::default();
        svc.install_container(container);

        let report = svc.probe(ProbeKind::Liveness).await;
        assert_eq!(report.status, IndicatorStatus::Up);
        assert!(report.details.is_empty());
    }
}
