use std::sync::OnceLock;
use std::time::Duration;

use nest_rs_core::{Container, ReachableProviders, injectable, inventory};

use crate::indicator::{HealthIndicator, IndicatorReport, IndicatorStatus, ProbeKind, ProbeReport};

/// Per-indicator wall-clock ceiling. An indicator that probes a dead peer (a
/// hung TCP connect, a stalled query) would otherwise block the public
/// `/health/ready` response indefinitely — a slow indicator must report `Down`,
/// not hang the whole probe (HEALTH-I7).
const INDICATOR_TIMEOUT: Duration = Duration::from_secs(5);

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
        // `Container` is a cheap `Arc` handle, so the clone is for the report's
        // borrow, not a second container.
        if self.container.set(container.clone()).is_ok() {
            report_unreachable_indicators(&container);
        }
    }

    /// Run every reachable indicator for `kind` and aggregate their results
    /// into a [`ProbeReport`]. Reports `up` if called before bootstrap wires
    /// the container, so a probe racing startup does not flap.
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
                // Silent here on purpose: `report_unreachable_indicators` named
                // it at boot, so repeating per probe would be the same event
                // said twice — once per request, in production.
                continue;
            }

            let (status, error) =
                run_with_timeout(entry.name, kind, (entry.run)(container), INDICATOR_TIMEOUT).await;
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

/// Name the linked-but-unreachable indicators **once, at boot** — the same
/// contract every other discovery seam honours (`nest_rs::queue`,
/// `nest_rs::events`): leftover code never vanishes silently, and the notice is
/// a startup fact rather than a line per probe. Called from
/// `install_container` because that is where the reachability set first exists;
/// probing then skips in silence.
fn report_unreachable_indicators(container: &Container) {
    // No reachability set means the access graph never ran (a hand-built
    // container in a test) — every indicator is then in scope, not skipped.
    let Some(reachable) = container.get::<ReachableProviders>() else {
        return;
    };
    for entry in inventory::iter::<HealthIndicator>() {
        if !reachable.0.contains(&(entry.provider_type_id)()) {
            report_inert_indicator(entry);
        }
    }
}

/// Report an inert indicator at the level its owner earns.
///
/// `debug` for a `nest-rs-*` capability the app never opted into — the
/// developer cannot act on it, and `nest_rs_seaorm`'s `db` / `db_ready` pair
/// warned twice per boot on two shipped demo apps, telling the reader to go
/// bind a framework-internal type. `warn` for the app's own leftover code,
/// which is what the module-gated discovery rule wants seen. Same call the
/// lifecycle runner makes, from the same place.
fn report_inert_indicator(entry: &HealthIndicator) {
    ::nest_rs_core::report_inert_host!(
        target: "nest_rs::health",
        what: "indicator",
        origin: entry.origin,
        indicator = entry.name,
        kind = ::tracing::field::debug(entry.kind),
    );
}

/// Run one indicator future under a wall-clock ceiling, mapping success,
/// failure, and timeout to a `(status, error)` pair. Extracted so the timeout
/// branch (HEALTH-I7) is testable without an inventory indicator. Public
/// probe responses carry only opaque reasons — never the indicator's internals.
async fn run_with_timeout(
    name: &'static str,
    kind: ProbeKind,
    fut: impl std::future::Future<Output = anyhow::Result<()>>,
    timeout: Duration,
) -> (IndicatorStatus, Option<String>) {
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(())) => (IndicatorStatus::Up, None),
        Ok(Err(err)) => {
            let detail = format!("{err:#}");
            tracing::warn!(
                target: "nest_rs::health",
                indicator = name,
                ?kind,
                error = %detail,
                "health indicator failed",
            );
            (IndicatorStatus::Down, Some("check failed".into()))
        }
        Err(_elapsed) => {
            tracing::warn!(
                target: "nest_rs::health",
                indicator = name,
                ?kind,
                timeout_secs = timeout.as_secs(),
                "health indicator timed out",
            );
            (IndicatorStatus::Down, Some("timed out".into()))
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

    #[tokio::test]
    async fn a_hanging_indicator_times_out_to_down() {
        // HEALTH-I7: an indicator probing a dead peer must not hang the probe.
        // A tiny real ceiling keeps the test fast while exercising the timeout
        // branch against a never-resolving future.
        let logs = nest_rs_testing::LogCapture::install();
        let (status, error) = run_with_timeout(
            "hang",
            ProbeKind::Readiness,
            std::future::pending::<anyhow::Result<()>>(),
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(status, IndicatorStatus::Down);
        assert_eq!(
            error.as_deref(),
            Some("timed out"),
            "a timed-out indicator reports Down with an opaque reason",
        );

        // The report says `Down` with an opaque reason — deliberately, since a
        // readiness body is served to whatever can reach the endpoint. So which
        // indicator hung, on which probe, and against what ceiling exists only
        // here, and an operator diagnosing a flapping readiness check has
        // nothing else to read.
        let event = logs.expect_one("nest_rs::health", "health indicator timed out");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("indicator").as_deref(), Some("hang"));
        assert_eq!(event.field("timeout_secs").as_deref(), Some("0"));
        assert!(
            event.field("kind").is_some_and(|k| k.contains("Readiness")),
            "the event names the probe that hung, got {:?}",
            event.fields,
        );
    }

    /// A `nest-rs-*` indicator the app never opted into reports at `debug`, not
    /// `warn`. `nest_rs_seaorm`'s `db` / `db_ready` pair warned twice on every
    /// boot of two shipped demo apps, telling the reader to bind a
    /// framework-internal type. Asserted on the emitted level rather than on
    /// `is_framework_owned`, which `nest-rs-core` already covers — this is the
    /// half only the health branch can be wrong about.
    #[test]
    fn a_framework_owned_indicator_reports_at_debug() {
        let logs = nest_rs_testing::LogCapture::install();
        // Driven directly rather than through a submitted fixture: `inventory`
        // is process-wide, so a second entry would join every other test in
        // this file.
        report_inert_indicator(&HealthIndicator {
            origin: "nest_rs_seaorm::health::indicator",
            name: "db",
            kind: ProbeKind::Readiness,
            provider_type_id: || std::any::TypeId::of::<UpHost>(),
            run: |_| Box::pin(async move { Ok(()) }),
        });

        let skipped = logs.find(
            "nest_rs::health",
            "skipped indicator: framework capability not imported by this app",
        );
        assert_eq!(skipped.len(), 1, "one line: {:#?}", logs.events());
        assert_eq!(skipped[0].level, "debug");
        assert_eq!(skipped[0].field("indicator").as_deref(), Some("db"));
    }

    #[tokio::test]
    async fn a_fast_indicator_is_not_affected_by_the_ceiling() {
        let (status, error) = run_with_timeout(
            "ok",
            ProbeKind::Readiness,
            async { Ok(()) },
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(status, IndicatorStatus::Up);
        assert!(error.is_none());
    }

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
            // App-shaped, deliberately: `module_path!()` here is
            // `nest_rs_health::…`, which `is_framework_owned` reads as the
            // framework's and reports at `debug`. These fixtures stand in for a
            // developer's own indicator, so they must say so.
            origin: "features::probes::up",
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
            origin: "features::probes::down",
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

    /// The two halves of a failed check, and where each is allowed to appear.
    /// They disagreed once — the body promised the operator a stringified
    /// error, the code shipped `"check failed"` — and the resolution is that
    /// `/health/*` is routinely unauthenticated, so the detail belongs in the
    /// log and nowhere else. Both directions are pinned: a future change that
    /// widens the body, *or* one that drops the log line, fails here.
    #[tokio::test]
    async fn the_indicators_own_error_reaches_the_log_and_never_the_body() {
        let logs = nest_rs_testing::LogCapture::install();
        let (status, error) = run_with_timeout(
            "migrations",
            ProbeKind::Startup,
            async { anyhow::bail!("pending migrations") },
            Duration::from_secs(5),
        )
        .await;

        assert_eq!(status, IndicatorStatus::Down);
        assert_eq!(
            error.as_deref(),
            Some("check failed"),
            "an unauthenticated probe body carries a fixed reason, never a DSN \
             or a hostname the anyhow chain picked up",
        );

        let event = logs.expect_one("nest_rs::health", "health indicator failed");
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("error").as_deref(),
            Some("pending migrations"),
            "…and the detail is one filtered log query away: {event:#?}",
        );
        assert_eq!(event.field("indicator").as_deref(), Some("migrations"));
    }

    /// A linked-but-unreachable indicator is named **once, at boot** — not on
    /// every probe. The line is a startup fact about the module tree; repeating
    /// it per request turns a wiring notice into production log volume, and no
    /// other discovery seam does that.
    #[tokio::test]
    async fn an_unreachable_indicator_is_named_once_at_boot_not_per_probe() {
        let container = Container::builder()
            .provide(UpHost)
            .provide(DownHost)
            // `DownHost` is deliberately absent from the reachable set: linked
            // into the binary, in no module the running app imports.
            .provide(ReachableProviders(
                [std::any::TypeId::of::<UpHost>()].into_iter().collect(),
            ))
            .build();

        let logs = nest_rs_testing::LogCapture::install();
        let svc = HealthService::default();
        svc.install_container(container);

        let skipped = logs.find(
            "nest_rs::health",
            "skipped indicator: no instance of the provider in this app's container",
        );
        assert_eq!(skipped.len(), 1, "one line at boot: {:#?}", logs.events());
        assert_eq!(skipped[0].level, "warn");
        assert_eq!(skipped[0].field("indicator").as_deref(), Some("down_host"));

        // Two probes later, still one line — and the unreachable indicator
        // never ran, so the probe is `up`.
        let report = svc.probe(ProbeKind::Readiness).await;
        let _ = svc.probe(ProbeKind::Readiness).await;
        assert_eq!(report.status, IndicatorStatus::Up);
        assert_eq!(
            logs.find(
                "nest_rs::health",
                "skipped indicator: no instance of the provider in this app's container",
            )
            .len(),
            1,
            "probing must not repeat the boot notice: {:#?}",
            logs.events(),
        );
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
